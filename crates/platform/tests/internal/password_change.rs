use super::*;
use crate::accounts::PasswordChange;
use std::sync::OnceLock;
fn change() -> PasswordChange {
    PasswordChange {
        current_password: "test-password-123".into(),
        new_password: "replacement-password".into(),
    }
}
#[tokio::test]
async fn replacement_revokes_only_owned_sessions_and_survives_restart() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let outsider = f.account("outsider").await;
    let group = f.group(&owner).await;
    let second: NodeToken = f
        .platform
        .login(credentials("owner"))
        .await
        .expect("second login")
        .try_into()
        .expect("token");
    assert_eq!(format!("{:?}", change()), "PasswordChange([redacted])");
    f.platform
        .change_password(&owner, change())
        .await
        .expect("change");
    assert!(f.platform.account(&owner).await.is_err());
    assert!(f.platform.account(&second).await.is_err());
    f.platform
        .account(&outsider)
        .await
        .expect("other account retained");
    assert!(f.platform.login(credentials("owner")).await.is_err());
    let saved: String =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE username='owner'")
            .fetch_one(&f.raw)
            .await
            .expect("hash");
    assert!(saved.starts_with("$argon2id$"));
    assert!(!saved.contains("replacement"));
    let reopened = Platform::open(&f.url, "http://127.0.0.1:8080")
        .await
        .expect("restart");
    let fresh: NodeToken = reopened
        .login(Credentials {
            password: "replacement-password".into(),
            ..credentials("owner")
        })
        .await
        .expect("new password")
        .try_into()
        .expect("token");
    assert_eq!(
        reopened
            .groups(&fresh, Uuid::nil())
            .await
            .expect("group retained")[0]
            .group_id,
        Uuid::from(group)
    );
    reopened.close().await;
    f.close().await;
}
#[tokio::test]
async fn invalid_changes_limits_and_worker_failure_leave_sessions_usable() {
    let mut f = Fixture::new().await;
    let token = f.account("owner").await;
    for input in [
        PasswordChange {
            current_password: "short".into(),
            ..change()
        },
        PasswordChange {
            new_password: "short".into(),
            ..change()
        },
        PasswordChange {
            new_password: "x".repeat(1025),
            ..change()
        },
    ] {
        assert!(f.platform.change_password(&token, input).await.is_err());
    }
    assert!(
        f.platform
            .change_password(&"f".repeat(64).try_into().expect("unknown"), change())
            .await
            .is_err()
    );
    assert!(
        f.platform
            .change_password(
                &token,
                PasswordChange {
                    current_password: "incorrect-password".into(),
                    ..change()
                }
            )
            .await
            .is_err()
    );
    let held = f
        .platform
        .password_slots
        .acquire_many(2)
        .await
        .expect("slots");
    assert!(matches!(
        f.platform.change_password(&token, change()).await,
        Err(Error::Limited)
    ));
    drop(held);
    f.platform.passwords.hash_password = |_| Err(Error::Password);
    assert!(f.platform.change_password(&token, change()).await.is_err());
    f.platform.passwords = crate::passwords::Passwords::default();
    f.platform.passwords.hash_worker = |_| {
        let task = tokio::spawn(std::future::pending());
        task.abort();
        task
    };
    assert!(f.platform.change_password(&token, change()).await.is_err());
    f.platform.passwords = crate::passwords::Passwords::default();
    f.sql("UPDATE accounts SET password_hash='corrupt',failed_logins=0")
        .await;
    assert!(f.platform.change_password(&token, change()).await.is_err());
    f.sql("UPDATE accounts SET failed_logins=5,login_window=clock_timestamp()")
        .await;
    assert!(f.platform.change_password(&token, change()).await.is_err());
    f.platform.account(&token).await.expect("session retained");
    f.platform.close().await;
    assert!(f.platform.change_password(&token, change()).await.is_err());
    f.close().await;
}
#[tokio::test]
async fn database_failures_roll_back_the_password_and_session_revocations() {
    for fault in [
        "CREATE TRIGGER fail BEFORE UPDATE ON accounts FOR EACH ROW WHEN (NEW.failed_logins=0) EXECUTE FUNCTION reject_write()",
        "CREATE TRIGGER fail BEFORE DELETE ON sessions FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "CREATE CONSTRAINT TRIGGER fail AFTER DELETE ON sessions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_write()",
    ] {
        let f = Fixture::new().await;
        let token = f.account("owner").await;
        let before: String = sqlx::query_scalar("SELECT password_hash FROM accounts")
            .fetch_one(&f.raw)
            .await
            .expect("hash");
        f.sql("CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$").await;
        f.sql(fault).await;
        assert!(f.platform.change_password(&token, change()).await.is_err());
        let after: String = sqlx::query_scalar("SELECT password_hash FROM accounts")
            .fetch_one(&f.raw)
            .await
            .expect("hash");
        assert_eq!(before, after);
        f.platform.account(&token).await.expect("retained session");
        f.close().await;
    }
}
static CHANGES: OnceLock<Arc<std::sync::Barrier>> = OnceLock::new();
#[tokio::test]
async fn concurrent_changes_cannot_both_replace_the_verified_password() {
    let mut f = Fixture::new().await;
    let token = f.account("owner").await;
    CHANGES
        .set(Arc::new(std::sync::Barrier::new(2)))
        .expect("barrier");
    f.platform.passwords.hash_worker = |job| {
        tokio::task::spawn_blocking(move || {
            CHANGES.get().expect("barrier").wait();
            job()
        })
    };
    let (a, b) = tokio::join!(
        f.platform.change_password(&token, change()),
        f.platform.change_password(&token, change())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(f.platform.account(&token).await.is_err());
    f.close().await;
}
static LOGIN: OnceLock<(tokio::sync::Notify, tokio::sync::Notify)> = OnceLock::new();
#[tokio::test]
async fn a_login_started_before_rotation_cannot_create_a_surviving_session() {
    let mut f = Fixture::new().await;
    let token = f.account("owner").await;
    LOGIN
        .set((tokio::sync::Notify::new(), tokio::sync::Notify::new()))
        .expect("signals");
    f.platform.passwords.verify_worker = |job| {
        tokio::spawn(async move {
            let (entered, release) = LOGIN.get().expect("signals");
            entered.notify_one();
            release.notified().await;
            tokio::task::spawn_blocking(job).await.expect("verify")
        })
    };
    let rotate = async {
        let (entered, release) = LOGIN.get().expect("signals");
        entered.notified().await;
        f.platform
            .change_password(&token, change())
            .await
            .expect("change");
        release.notify_one();
    };
    let (login, ()) = tokio::join!(f.platform.login(credentials("owner")), rotate);
    assert!(matches!(login, Err(Error::Unauthorized)));
    f.close().await;
}
