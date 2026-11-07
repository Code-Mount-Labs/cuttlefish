use super::*;
use crate::{auth::Credentials, settings::ModelSettings};
fn credentials() -> Credentials {
    Credentials {
        username: "operator".into(),
        password: "long-enough-password".into(),
    }
}
async fn sql(machine: &Machine, sql: &'static str) {
    sqlx::raw_sql(sql)
        .execute(&machine.pool)
        .await
        .expect("fault injection");
}
#[tokio::test]
async fn closed_journal_errors_propagate_without_authorizing_or_starting_work() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    let token = machine.bootstrap_token().expect("token");
    machine.close().await;
    assert!(machine.initialized().await.is_err());
    assert!(machine.identity().await.is_err());
    assert!(machine.status().await.is_err());
    assert!(
        machine
            .initialize(
                &registration(),
                "http://127.0.0.1:8081",
                "http://127.0.0.1:8080"
            )
            .await
            .is_err()
    );
    assert!(machine.setup_admin(&token, credentials()).await.is_err());
    assert!(machine.login(credentials()).await.is_err());
    assert!(machine.authenticate(&token).await.is_err());
    assert!(machine.logout(&token).await.is_err());
    assert!(
        machine
            .select_database(DatabaseChoice::Later)
            .await
            .is_err()
    );
    assert!(machine.configure(DatabaseChoice::Later).await.is_err());
    assert!(machine.retry_database().await.is_err());
    assert!(machine.recover().await.is_err());
    assert!(
        machine
            .configure_model(&ModelSettings {
                endpoint: "https://example.com".into(),
                api_key: "key".into(),
                model: "model".into()
            })
            .await
            .is_err()
    );
    assert!(machine.model_status().await.is_err());
    assert!(machine.router_origin().await.is_err());
    assert!(machine.register().await.is_err());
    assert!(machine.advertise(false).await.is_err());
    assert!(
        machine
            .authorize_agent(registration().installation_id, &token)
            .await
            .is_err()
    );
    assert!(machine.execution_store().await.is_err());
}
#[tokio::test]
async fn administrator_transaction_rolls_back_on_write_and_deferred_commit_failures() {
    for trigger in [
        "CREATE TRIGGER fail BEFORE INSERT ON local_admin BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON local_sessions BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail AFTER INSERT ON local_sessions BEGIN INSERT INTO child VALUES(99); END",
    ] {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
            .await
            .expect("machine");
        initialize(&machine).await;
        let token = machine.bootstrap_token().expect("token");
        sql(&machine, trigger).await;
        assert!(machine.setup_admin(&token, credentials()).await.is_err());
        assert!(!machine.initialized().await.expect("not created"));
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM local_sessions")
            .fetch_one(&machine.pool)
            .await
            .expect("count");
        assert_eq!(sessions, 0);
        machine.close().await;
    }
}
#[tokio::test]
async fn login_failures_never_mint_a_session() {
    for trigger in [
        "CREATE TRIGGER fail BEFORE UPDATE ON local_admin WHEN NEW.failed_logins=0 BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE DELETE ON local_sessions BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER fail BEFORE INSERT ON local_sessions BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail AFTER INSERT ON local_sessions BEGIN INSERT INTO child VALUES(99); END",
    ] {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
            .await
            .expect("machine");
        initialize(&machine).await;
        machine
            .setup_admin(&machine.bootstrap_token().expect("token"), credentials())
            .await
            .expect("admin");
        sql(&machine, "UPDATE local_sessions SET expires_at=0").await;
        sql(&machine, trigger).await;
        assert!(machine.login(credentials()).await.is_err());
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM local_sessions WHERE expires_at>unixepoch()")
                .fetch_one(&machine.pool)
                .await
                .expect("count");
        assert_eq!(count, 0);
        machine.close().await;
    }
}
#[tokio::test]
async fn corrupt_resource_state_and_failed_writes_never_report_ready() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    assert!(machine.retry_database().await.is_err());
    sql(&machine, "UPDATE resources SET spec='{}'").await;
    assert!(machine.status().await.is_err());
    assert!(machine.recover().await.is_err());
    sql(&machine,"UPDATE resources SET spec=NULL; CREATE TRIGGER fail BEFORE UPDATE ON resources BEGIN SELECT RAISE(ABORT,'injected'); END").await;
    assert!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://host/db".into()
            })
            .await
            .is_err()
    );
    sql(&machine, "DROP TRIGGER fail").await;
    machine
        .select_database(DatabaseChoice::Existing {
            url: "postgresql://host/db".into(),
        })
        .await
        .expect("selection");
    sql(
        &machine,
        "CREATE TRIGGER fail BEFORE UPDATE ON resources BEGIN SELECT RAISE(ABORT,'injected'); END",
    )
    .await;
    assert!(machine.retry_database().await.is_err());
    assert!(machine.recover().await.is_err());
    sql(&machine,"DROP TRIGGER fail; CREATE TRIGGER fail BEFORE UPDATE ON resources WHEN NEW.state='ready' BEGIN SELECT RAISE(ABORT,'injected'); END").await;
    assert!(machine.recover().await.is_err());
    assert_eq!(
        machine.status().await.expect("status").state,
        "provisioning"
    );
    machine.close().await;
}
#[tokio::test]
async fn invalid_identity_and_password_worker_failure_are_not_hidden() {
    let dir = tempfile::tempdir().expect("directory");
    let mut machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(
        machine
            .initialize(&registration(), "invalid", "http://127.0.0.1:8080")
            .await
            .is_err()
    );
    assert!(
        machine
            .initialize(&registration(), "http://127.0.0.1:8081", "invalid")
            .await
            .is_err()
    );
    initialize(&machine).await;
    sql(&machine, "UPDATE installation SET registration='{}'").await;
    assert!(machine.identity().await.is_err());
    machine.passwords.hash_password = |_| Err(Error::Dependency("password_hashing_failed"));
    assert!(
        machine
            .setup_admin(&machine.bootstrap_token().expect("token"), credentials())
            .await
            .is_err()
    );
    machine.passwords = crate::passwords::Passwords::default();
    machine
        .setup_admin(&machine.bootstrap_token().expect("token"), credentials())
        .await
        .expect("admin");
    machine.passwords.verify_worker = |_| {
        let job = tokio::spawn(std::future::pending());
        job.abort();
        job
    };
    assert!(machine.login(credentials()).await.is_err());
    assert!(
        machine
            .login(Credentials {
                username: "a".into(),
                password: "b".into()
            })
            .await
            .is_err()
    );
    machine.close().await;
}

#[tokio::test]
async fn setup_file_failures_never_authorize_an_administrator() {
    let dir = tempfile::tempdir().expect("directory");
    let mut machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    machine.files.create = |_| Err(std::io::Error::other("disk full"));
    assert!(machine.bootstrap_token().is_err());
    assert!(
        machine
            .setup_admin(&registration().credential, credentials())
            .await
            .is_err()
    );
    machine.files = crate::files::AtomicFiles::default();
    std::fs::create_dir(dir.path().join("setup.token")).expect("unreadable token");
    assert!(machine.bootstrap_token().is_err());
    assert!(!machine.initialized().await.expect("not initialized"));
    machine.close().await;
}
#[tokio::test]
async fn incomplete_or_corrupt_ready_state_does_not_open_execution() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    sql(&machine, "UPDATE resources SET state='ready',spec=NULL").await;
    assert!(matches!(
        machine.execution_store().await,
        Err(Error::Invalid("ready_database_has_no_configuration"))
    ));
    sql(&machine, "DROP TABLE resources").await;
    assert!(machine.execution_store().await.is_err());
    sql(&machine, "DROP TABLE local_settings").await;
    assert!(machine.save_setting("test", "value").await.is_err());
    machine.close().await;
}
#[derive(Debug)]
struct BrokenBackend;
impl DatabaseBackend for BrokenBackend {
    fn reconcile<'a>(&'a self, _: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async { Err(std::io::Error::other("injected backend failure").into()) })
    }
}
#[tokio::test]
async fn unexpected_backend_failure_is_persisted_without_private_details() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(BrokenBackend))
        .await
        .expect("machine");
    initialize(&machine).await;
    assert!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://host/db".into()
            })
            .await
            .is_err()
    );
    let status = machine.status().await.expect("status");
    assert_eq!(status.state, "failed");
    assert!(
        serde_json::to_string(&status)
            .expect("json")
            .contains("database_setup_failed")
    );
    machine.close().await;
}

#[derive(Debug)]
struct InvalidBackend;
impl DatabaseBackend for InvalidBackend {
    fn reconcile<'a>(&'a self, _: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async { Err(Error::Invalid("resource_ownership_mismatch")) })
    }
}
#[tokio::test]
async fn invalid_resource_configuration_is_not_reported_as_ready() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(InvalidBackend))
        .await
        .expect("machine");
    assert!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://host/db".into()
            })
            .await
            .is_err()
    );
    assert_eq!(
        machine
            .status()
            .await
            .expect("status")
            .error_code
            .as_deref(),
        Some("resource_ownership_mismatch")
    );
    machine.close().await;
}
