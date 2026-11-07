use super::*;
#[tokio::test]
async fn failed_session_writes_and_commits_leave_accounts_unchanged() {
    let f = Fixture::new().await;
    f.account("owner").await;
    f.sql("CREATE FUNCTION refuse_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$").await;
    f.sql("CREATE TRIGGER refuse_write BEFORE INSERT ON sessions FOR EACH ROW EXECUTE FUNCTION refuse_write()").await;
    assert!(
        f.platform
            .register_account(credentials("another"))
            .await
            .is_err()
    );
    assert!(f.platform.login(credentials("owner")).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&f.raw)
        .await
        .expect("count");
    assert_eq!(count, 1);
    f.sql("DROP TRIGGER refuse_write ON sessions; CREATE CONSTRAINT TRIGGER refuse_commit AFTER INSERT ON sessions DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_write()").await;
    assert!(
        f.platform
            .register_account(credentials("another"))
            .await
            .is_err()
    );
    assert!(f.platform.login(credentials("owner")).await.is_err());
    f.sql("DROP TRIGGER refuse_commit ON sessions; CREATE TRIGGER refuse_reset BEFORE UPDATE ON accounts FOR EACH ROW WHEN (NEW.failed_logins=0) EXECUTE FUNCTION refuse_write()").await;
    assert!(f.platform.login(credentials("owner")).await.is_err());
    f.sql("DROP TRIGGER refuse_reset ON accounts; ALTER TABLE sessions RENAME TO unavailable_sessions").await;
    assert!(f.platform.login(credentials("owner")).await.is_err());
    f.sql("ALTER TABLE unavailable_sessions RENAME TO sessions")
        .await;
    f.close().await;
}
#[tokio::test]
async fn registry_query_and_commit_failures_preserve_invitations() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let input = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::RouterAgent,
        900,
    );
    f.sql("ALTER TABLE app_groups RENAME TO unavailable_groups")
        .await;
    assert!(f.platform.create_group(&owner, "name").await.is_err());
    assert!(f.platform.groups(&owner, Uuid::nil()).await.is_err());
    assert!(f.platform.invite(&owner, group).await.is_err());
    assert!(f.platform.register_installation(&input).await.is_err());
    f.sql("ALTER TABLE unavailable_groups RENAME TO app_groups; ALTER TABLE installations RENAME TO unavailable_installations").await;
    assert!(
        f.platform
            .installations(&owner, group, Uuid::nil())
            .await
            .is_err()
    );
    assert!(
        f.platform
            .preflight(group, &input.invitation)
            .await
            .is_err()
    );
    assert!(f.platform.register_installation(&input).await.is_err());
    f.sql("ALTER TABLE unavailable_installations RENAME TO installations; ALTER TABLE invitations RENAME TO unavailable_invitations").await;
    assert!(f.platform.invite(&owner, group).await.is_err());
    assert!(f.platform.register_installation(&input).await.is_err());
    f.sql("ALTER TABLE unavailable_invitations RENAME TO invitations; CREATE FUNCTION refuse_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER refuse_write BEFORE UPDATE ON invitations FOR EACH ROW EXECUTE FUNCTION refuse_write()").await;
    assert!(f.platform.register_installation(&input).await.is_err());
    f.sql("DROP TRIGGER refuse_write ON invitations; CREATE CONSTRAINT TRIGGER refuse_commit AFTER INSERT ON installations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_write()").await;
    assert!(f.platform.register_installation(&input).await.is_err());
    assert_eq!(
        f.platform
            .preflight(group, &input.invitation)
            .await
            .expect("preserved")
            .roles
            .len(),
        2
    );
    f.sql("DROP TRIGGER refuse_commit ON installations").await;
    f.platform
        .register_installation(&input)
        .await
        .expect("retry");
    f.sql("CREATE TRIGGER refuse_write BEFORE UPDATE ON installations FOR EACH ROW EXECUTE FUNCTION refuse_write()").await;
    assert!(
        f.platform
            .heartbeat(&input.credential, &heartbeat(input.role))
            .await
            .is_err()
    );
    f.sql("ALTER TABLE installations RENAME TO unavailable_installations")
        .await;
    assert!(f.platform.register_installation(&input).await.is_err());
    f.sql(
        "ALTER TABLE unavailable_installations RENAME TO installations; DELETE FROM installations",
    )
    .await;
    assert!(f.platform.register_installation(&input).await.is_err());
    f.close().await;
}
#[tokio::test]
async fn startup_rejects_invalid_origins_and_migration_conflicts() {
    assert!(Platform::open("not a URL", "not a URL").await.is_err());
    assert!(
        Platform::open("not a URL", "https://platform.test")
            .await
            .is_err()
    );
    let f = Fixture::new().await;
    f.sql("UPDATE _sqlx_migrations SET checksum=decode('00','hex')")
        .await;
    assert!(matches!(
        Platform::open(&f.url, "https://platform.test").await,
        Err(Error::Migration(_))
    ));
    f.close().await;
}

async fn fail_second_checkout(url: &str) -> PgPool {
    use std::sync::atomic::{AtomicBool, Ordering};
    let used = Arc::new(AtomicBool::new(false));
    let unavailable = Arc::new(AtomicBool::new(false));
    let connect_unavailable = unavailable.clone();
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .after_connect(move |_, _| {
            let unavailable = connect_unavailable.load(Ordering::SeqCst);
            Box::pin(async move {
                if unavailable {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(())
                }
            })
        })
        .before_acquire(move |_, _| {
            let reject = used.swap(true, Ordering::SeqCst);
            if reject {
                unavailable.store(true, Ordering::SeqCst);
            }
            Box::pin(async move { Ok(!reject) })
        })
        .connect(url)
        .await
        .expect("controlled pool")
}
#[tokio::test]
async fn losing_database_between_validation_and_transaction_fails_closed() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let invitation = f.invitation(&owner, group).await;
    let mut replica = f.platform.clone();
    replica.pool = fail_second_checkout(&f.url).await;
    assert!(matches!(
        replica.preflight(group, &invitation).await,
        Err(Error::Database(sqlx::Error::PoolTimedOut))
    ));
    replica.close().await;
    replica.pool = fail_second_checkout(&f.url).await;
    assert!(matches!(
        replica.login(credentials("owner")).await,
        Err(Error::Database(sqlx::Error::PoolTimedOut))
    ));
    replica.close().await;
    f.close().await;
}
