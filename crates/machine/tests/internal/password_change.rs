use crate::{
    Error,
    auth::{Credentials, PasswordChange},
    state::Machine,
};
use orchestrator_protocol::sync::NodeToken;
use std::sync::{Arc, OnceLock};

fn credentials() -> Credentials {
    Credentials {
        username: "operator".into(),
        password: "original-password".into(),
    }
}
fn change() -> PasswordChange {
    PasswordChange {
        current_password: "original-password".into(),
        new_password: "replacement-password".into(),
    }
}
async fn fixture() -> (tempfile::TempDir, Machine, NodeToken) {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(crate::tests::Backend::default()))
        .await
        .expect("machine");
    crate::tests::initialize(&machine).await;
    let token = machine
        .setup_admin(&machine.bootstrap_token().expect("setup"), credentials())
        .await
        .expect("admin")
        .try_into()
        .expect("token");
    (dir, machine, token)
}
async fn sql(machine: &Machine, text: &'static str) {
    sqlx::raw_sql(text)
        .execute(&machine.pool)
        .await
        .expect("test SQL");
}

#[tokio::test]
async fn replacement_revokes_all_sessions_and_survives_restart() {
    let (dir, machine, token) = fixture().await;
    let second: NodeToken = machine
        .login(credentials())
        .await
        .expect("second login")
        .try_into()
        .expect("token");
    assert_eq!(format!("{:?}", change()), "PasswordChange([redacted])");
    machine
        .change_password(&token, change())
        .await
        .expect("change");
    assert!(machine.authenticate(&token).await.is_err());
    assert!(machine.authenticate(&second).await.is_err());
    assert!(machine.login(credentials()).await.is_err());
    let saved: String = sqlx::query_scalar("SELECT password_hash FROM local_admin")
        .fetch_one(&machine.pool)
        .await
        .expect("hash");
    assert!(saved.starts_with("$argon2id$"));
    assert!(!saved.contains("replacement"));
    machine.close().await;
    drop(machine);
    let reopened = Machine::open(dir.path(), Arc::new(crate::tests::Backend::default()))
        .await
        .expect("restart");
    let fresh = reopened
        .login(Credentials {
            password: "replacement-password".into(),
            ..credentials()
        })
        .await
        .expect("new password");
    reopened
        .authenticate(&fresh.try_into().expect("token"))
        .await
        .expect("new session");
    reopened.close().await;
}

#[tokio::test]
async fn invalid_credentials_limits_and_corrupt_hash_leave_password_unchanged() {
    let (_dir, mut machine, token) = fixture().await;
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
        assert!(machine.change_password(&token, input).await.is_err());
    }
    assert!(
        machine
            .change_password(&"f".repeat(64).try_into().expect("unknown"), change())
            .await
            .is_err()
    );
    assert!(
        machine
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
    let held = machine.password_slots.acquire_many(2).await.expect("slots");
    assert!(matches!(
        machine.change_password(&token, change()).await,
        Err(Error::Limited)
    ));
    drop(held);
    machine.passwords.hash_password = |_| Err(Error::Dependency("injected"));
    assert!(machine.change_password(&token, change()).await.is_err());
    machine.passwords = crate::passwords::Passwords::default();
    machine.passwords.hash_worker = |_| {
        let task = tokio::spawn(std::future::pending());
        task.abort();
        task
    };
    assert!(machine.change_password(&token, change()).await.is_err());
    machine.passwords = crate::passwords::Passwords::default();
    sql(
        &machine,
        "UPDATE local_admin SET password_hash='corrupt',failed_logins=0",
    )
    .await;
    assert!(machine.change_password(&token, change()).await.is_err());
    sql(
        &machine,
        "UPDATE local_admin SET failed_logins=5,login_window=unixepoch()",
    )
    .await;
    assert!(machine.change_password(&token, change()).await.is_err());
    machine.close().await;
    assert!(machine.change_password(&token, change()).await.is_err());
}

#[tokio::test]
async fn password_change_rolls_back_on_update_delete_and_commit_failure() {
    for fault in [
        "CREATE TRIGGER fail BEFORE UPDATE ON local_admin WHEN NEW.failed_logins=0 BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE DELETE ON local_sessions BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail AFTER DELETE ON local_sessions BEGIN INSERT INTO child VALUES(1); END",
    ] {
        let (_dir, machine, token) = fixture().await;
        let previous: String = sqlx::query_scalar("SELECT password_hash FROM local_admin")
            .fetch_one(&machine.pool)
            .await
            .expect("hash");
        sql(&machine, fault).await;
        assert!(machine.change_password(&token, change()).await.is_err());
        let saved: String = sqlx::query_scalar("SELECT password_hash FROM local_admin")
            .fetch_one(&machine.pool)
            .await
            .expect("hash");
        assert_eq!(saved, previous);
        machine
            .authenticate(&token)
            .await
            .expect("session retained");
        machine.close().await;
    }
}

static CHANGES: OnceLock<Arc<std::sync::Barrier>> = OnceLock::new();
#[tokio::test]
async fn concurrent_old_password_changes_have_only_one_winner() {
    let (_dir, mut machine, token) = fixture().await;
    CHANGES
        .set(Arc::new(std::sync::Barrier::new(2)))
        .expect("barrier");
    machine.passwords.hash_worker = |job| {
        tokio::task::spawn_blocking(move || {
            CHANGES.get().expect("barrier").wait();
            job()
        })
    };
    let (a, b) = tokio::join!(
        machine.change_password(&token, change()),
        machine.change_password(&token, change())
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(machine.authenticate(&token).await.is_err());
    machine.close().await;
}

static LOGIN: OnceLock<(tokio::sync::Notify, tokio::sync::Notify)> = OnceLock::new();
#[tokio::test]
async fn in_flight_old_password_login_cannot_escape_session_revocation() {
    let (_dir, mut machine, token) = fixture().await;
    LOGIN
        .set((tokio::sync::Notify::new(), tokio::sync::Notify::new()))
        .expect("signals");
    machine.passwords.verify_worker = |job| {
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
        machine
            .change_password(&token, change())
            .await
            .expect("change");
        release.notify_one();
    };
    let (login, ()) = tokio::join!(machine.login(credentials()), rotate);
    assert!(matches!(login, Err(Error::Unauthorized)));
    machine.close().await;
}

#[tokio::test]
async fn password_endpoint_requires_origin_session_and_current_password() {
    use crate::tests::http::request;
    use axum::http::StatusCode;
    let (_dir, machine, token) = fixture().await;
    let app = crate::web::router(machine.clone());
    let cookie = format!("cuttlefish_local_session={}", token.expose());
    let body = serde_json::json!({"current_password":"original-password","new_password":"replacement-password"});
    for (cookie, origin) in [
        (None, None),
        (Some(cookie.as_str()), None),
        (Some(cookie.as_str()), Some("https://attacker.test")),
    ] {
        let (status, _, _) = request(
            &app,
            "/api/local/password",
            Some(body.clone()),
            cookie,
            origin,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status,_,_)=request(&app,"/api/local/password",Some(serde_json::json!({"current_password":"incorrect-password","new_password":"replacement-password"})),Some(&cookie),Some("http://127.0.0.1:8080")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, cookie) = request(
        &app,
        "/api/local/password",
        Some(body),
        Some(&cookie),
        Some("http://127.0.0.1:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.expect("cookie").contains("Max-Age=0"));
    machine.close().await;
}

static CLOSING_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
#[tokio::test]
async fn losing_database_after_password_verification_fails_closed() {
    let (dir, mut machine, token) = fixture().await;
    CLOSING_POOL.set(machine.pool.clone()).expect("pool");
    machine.passwords.hash_worker = |job| {
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(job).await?;
            CLOSING_POOL.get().expect("pool").close().await;
            result
        })
    };
    assert!(matches!(
        machine.change_password(&token, change()).await,
        Err(Error::Database(sqlx::Error::PoolClosed))
    ));
    drop(machine);
    let reopened = Machine::open(dir.path(), Arc::new(crate::tests::Backend::default()))
        .await
        .expect("restart");
    reopened.authenticate(&token).await.expect("old session");
    reopened.login(credentials()).await.expect("old password");
    reopened.close().await;
}
