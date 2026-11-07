#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{Backend, registration};
use axum::{Json, Router, http::StatusCode, routing::post};
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
#[derive(Debug, Default)]
struct Worker {
    fail: AtomicBool,
    fail_stop: AtomicBool,
    calls: Mutex<Vec<String>>,
}
impl Connector for Worker {
    fn start<'a>(
        &'a self,
        lease: &'a RelayLease,
        credential: &'a str,
        _: &'a str,
    ) -> ConnectorFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("lock")
                .push(format!("{}:{credential}", lease.epoch));
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::Dependency("injected_start_failure"))
            } else {
                Ok(())
            }
        })
    }
    fn stop(&self, _: InstallationId) -> ConnectorFuture<'_> {
        Box::pin(async move {
            self.calls.lock().expect("lock").push("stop".into());
            if self.fail_stop.load(Ordering::SeqCst) {
                Err(Error::Dependency("stop failed"))
            } else {
                Ok(())
            }
        })
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    machine: Machine,
    worker: Arc<Worker>,
    proof: Arc<Mutex<Value>>,
    grants: Arc<Mutex<Vec<RelayMutation>>>,
    fail: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
    origin: String,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let worker = Arc::new(Worker::default());
        let mut machine = Machine::open(directory.path(), Arc::new(Backend::default()))
            .await
            .expect("machine");
        machine.connector = worker.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let proof = Arc::new(Mutex::new(json!({})));
        let grants = Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let captured = grants.clone();
        let endpoint = origin.clone();
        let failing = fail.clone();
        let serving = proof.clone();
        let serving_root = proof.clone();
        let app = Router::new()
            .route(
                "/api/v1/installations/relay",
                post(move |Json(input): Json<RelayMutation>| {
                    let captured = captured.clone();
                    let endpoint = endpoint.clone();
                    async move {
                        captured.lock().expect("lock").push(input.clone());
                        Json(RelayLease {
                            installation_id: registration().installation_id,
                            epoch: Uuid::from_u128(
                                u128::try_from(input.revision).expect("positive"),
                            ),
                            public_url: format!("{endpoint}/i/{}", registration().installation_id),
                            issuer: endpoint,
                        })
                    }
                }),
            )
            .route(
                "/api/v1/installations/address",
                post(move || {
                    let failing = failing.clone();
                    async move {
                        (
                            if failing.load(Ordering::SeqCst) {
                                StatusCode::SERVICE_UNAVAILABLE
                            } else {
                                StatusCode::OK
                            },
                            Json(json!({"ok":true})),
                        )
                    }
                }),
            )
            .route(
                "/i/{id}/api/local/relay-proof",
                post(move || {
                    let serving = serving.clone();
                    async move { Json(serving.lock().expect("lock").clone()) }
                }),
            )
            .route(
                "/api/local/relay-proof",
                post(move || {
                    let serving = serving_root.clone();
                    async move { Json(serving.lock().expect("lock").clone()) }
                }),
            );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server");
        });
        machine
            .initialize(&registration(), &origin, "http://127.0.0.1:8080")
            .await
            .expect("initialize");
        machine
            .save_setting("registered", "true")
            .await
            .expect("registered");
        machine
            .set_local_endpoint("http://127.0.0.1:8080")
            .await
            .expect("listener");
        Self {
            directory,
            machine,
            worker,
            proof,
            grants,
            fail,
            task,
            origin,
        }
    }
    async fn admin(&self) {
        self.machine
            .setup_admin(
                &self.machine.bootstrap_token().expect("bootstrap"),
                crate::auth::Credentials {
                    username: "relay.owner".into(),
                    password: "long-safe-test-password".into(),
                },
            )
            .await
            .expect("admin");
    }
    async fn select(&self, choice: RemoteChoice) {
        self.machine.select_remote(choice).await.expect("select");
        *self.proof.lock().expect("lock") = self.machine.remote_proof().await.expect("proof");
    }
}
#[tokio::test]
async fn acquisition_and_publication_recover_without_changing_credentials() {
    let mut f = Fixture::new().await;
    f.select(RemoteChoice::Managed {}).await;
    assert_eq!(
        f.machine.recover_remote().await.expect("wait").state,
        "waiting_for_admin"
    );
    assert!(f.worker.calls.lock().expect("lock").is_empty());
    f.admin().await;
    f.worker.fail.store(true, Ordering::SeqCst);
    assert!(f.machine.recover_remote().await.is_err());
    assert_eq!(
        f.machine
            .remote_status()
            .await
            .expect("status")
            .error_code
            .as_deref(),
        Some("injected_start_failure")
    );
    let original = f.machine.remote_saved().await.expect("saved");
    f.machine.close().await;
    drop(f.machine);
    let mut reopened = Machine::open(f.directory.path(), Arc::new(Backend::default()))
        .await
        .expect("restart");
    reopened.connector = f.worker.clone();
    f.machine = reopened;
    f.worker.fail.store(false, Ordering::SeqCst);
    f.fail.store(true, Ordering::SeqCst);
    assert!(f.machine.recover_remote().await.is_err());
    f.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        f.machine
            .recover_remote()
            .await
            .expect("recover publication")
            .state,
        "ready"
    );
    let restored = f.machine.remote_saved().await.expect("saved");
    assert_eq!(original.credential, restored.credential);
    assert_eq!(
        original.lease.expect("lease").epoch,
        restored.lease.expect("lease").epoch
    );
    let calls = f.worker.calls.lock().expect("lock").clone();
    assert!(calls.iter().all(|call| call == &calls[0]));
    assert!(
        f.grants
            .lock()
            .expect("lock")
            .iter()
            .all(|request| request.revision == 1)
    );
    f.select(RemoteChoice::Local {}).await;
    assert_eq!(
        f.machine.recover_remote().await.expect("disable").state,
        "local"
    );
    assert!(
        f.grants
            .lock()
            .expect("lock")
            .last()
            .expect("revoke")
            .credential
            .is_none()
    );
    assert_eq!(
        f.machine
            .recover_remote()
            .await
            .expect("idempotent local")
            .state,
        "local"
    );
    assert!(f.machine.remote_origin().await.expect("origin").is_none());
    f.machine.close().await;
    f.task.abort();
}
fn http_fixture(value: &str) -> Result<crate::registry::Client, Error> {
    crate::registry::Client::new(&value.replacen("https://", "http://", 1))
}
#[tokio::test]
async fn existing_address_is_verified_and_pending_failures_do_not_publish() {
    let mut f = Fixture::new().await;
    f.admin().await;
    assert!(
        f.machine
            .select_remote(RemoteChoice::Existing {
                origin: f.origin.clone()
            })
            .await
            .is_err()
    );
    f.machine.remote_client = http_fixture;
    let https = f.origin.replacen("http://", "https://", 1);
    f.select(RemoteChoice::Existing {
        origin: https.clone(),
    })
    .await;
    assert_eq!(
        f.machine
            .remote_origin()
            .await
            .expect("pending origin")
            .as_deref(),
        Some(https.as_str())
    );
    *f.proof.lock().expect("lock") =
        json!({"installation_id":registration().installation_id,"proof":"wrong"});
    assert!(f.machine.recover_remote().await.is_err());
    assert!(
        f.machine
            .remote_status()
            .await
            .expect("status")
            .public_url
            .is_none()
    );
    *f.proof.lock().expect("lock") = f.machine.remote_proof().await.expect("proof");
    assert_eq!(
        f.machine
            .recover_remote()
            .await
            .expect("verified")
            .public_url
            .as_deref(),
        Some(https.as_str())
    );
    assert!(f.grants.lock().expect("lock").is_empty());
    f.machine.close().await;
    f.task.abort();
}
#[tokio::test]
async fn corrupt_or_unavailable_journal_and_invalid_targets_fail_explicitly() {
    let f = Fixture::new().await;
    for target in [
        "invalid",
        "https://127.0.0.1",
        "http://192.0.2.1",
        "http://127.0.0.1/path",
    ] {
        assert!(f.machine.set_local_endpoint(target).await.is_err());
    }
    assert!(
        f.machine
            .select_remote(RemoteChoice::Existing {
                origin: "invalid".into()
            })
            .await
            .is_err()
    );
    f.admin().await;
    f.select(RemoteChoice::Managed {}).await;
    sqlx::query("UPDATE remote_access SET credential=NULL")
        .execute(&f.machine.pool)
        .await
        .expect("corrupt");
    assert!(f.machine.recover_remote().await.is_err());
    sqlx::query("UPDATE remote_access SET local_origin=NULL")
        .execute(&f.machine.pool)
        .await
        .expect("corrupt");
    assert!(f.machine.recover_remote().await.is_err());
    f.machine.close().await;
    assert!(f.machine.remote_status().await.is_err());
    assert!(f.machine.remote_proof().await.is_err());
    assert!(
        f.machine
            .select_remote(RemoteChoice::Managed {})
            .await
            .is_err()
    );
    f.task.abort();
}

async fn sql(machine: &Machine, statement: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&machine.pool)
        .await
        .expect("fixture SQL");
}
#[tokio::test]
async fn journal_faults_do_not_publish_or_lose_pending_remote_changes() {
    let f = Fixture::new().await;
    f.select(RemoteChoice::Managed {}).await;
    sql(&f.machine,"CREATE TRIGGER fail BEFORE UPDATE ON remote_access BEGIN SELECT RAISE(ABORT,'write failure'); END").await;
    assert!(f.machine.recover_remote().await.is_err());
    assert!(
        f.machine
            .set_local_endpoint("http://127.0.0.1:9999")
            .await
            .is_err()
    );
    sql(&f.machine, "DROP TRIGGER fail").await;
    f.admin().await;
    sql(&f.machine,"CREATE TRIGGER fail BEFORE UPDATE OF lease ON remote_access BEGIN SELECT RAISE(ABORT,'lease failure'); END").await;
    assert!(f.machine.recover_remote().await.is_err());
    assert_eq!(
        f.machine
            .remote_status()
            .await
            .expect("status")
            .error_code
            .as_deref(),
        Some("remote_setup_failed")
    );
    sql(&f.machine,"DROP TRIGGER fail; CREATE TRIGGER fail BEFORE UPDATE ON remote_access WHEN NEW.state='ready' BEGIN SELECT RAISE(ABORT,'publication failure'); END").await;
    assert!(f.machine.recover_remote().await.is_err());
    assert!(
        f.machine
            .remote_origin()
            .await
            .expect("managed origin")
            .is_some()
    );
    sql(
        &f.machine,
        "DROP TRIGGER fail; UPDATE remote_access SET revision=9223372036854775807",
    )
    .await;
    assert!(matches!(
        f.machine.select_remote(RemoteChoice::Local {}).await,
        Err(Error::Invalid("remote_revision_exhausted"))
    ));
    let saved = f.machine.remote_saved().await.expect("saved");
    sql(
        &f.machine,
        "ALTER TABLE local_settings RENAME TO hidden_settings",
    )
    .await;
    assert!(f.machine.reconcile_remote(&saved).await.is_err());
    sql(&f.machine,"ALTER TABLE hidden_settings RENAME TO local_settings; DELETE FROM local_settings WHERE name='registered'").await;
    assert!(f.machine.reconcile_remote(&saved).await.is_err());
    sql(
        &f.machine,
        "ALTER TABLE installation RENAME TO hidden_installation",
    )
    .await;
    assert!(f.machine.reconcile_remote(&saved).await.is_err());
    sql(&f.machine,"ALTER TABLE hidden_installation RENAME TO installation; ALTER TABLE local_admin RENAME TO hidden_admin").await;
    assert!(f.machine.reconcile_remote(&saved).await.is_err());
    sql(&f.machine, "DROP TABLE remote_access").await;
    assert!(f.machine.remote_origin().await.is_err());
    assert!(f.machine.remote_proof().await.is_err());
    assert!(f.machine.recover_remote().await.is_err());
    f.task.abort();
    f.machine.close().await;
}
fn broken_client(_: &str) -> Result<crate::registry::Client, Error> {
    Err(Error::Dependency("test client unavailable"))
}
fn broken_proof_client(value: &str) -> Result<crate::registry::Client, Error> {
    if value.contains("/i/") {
        broken_client(value)
    } else {
        crate::registry::Client::new(value)
    }
}
#[tokio::test]
async fn connection_failures_and_wrong_lease_identity_never_publish() {
    let mut f = Fixture::new().await;
    f.admin().await;
    f.select(RemoteChoice::Managed {}).await;
    f.machine.remote_client = broken_client;
    assert!(f.machine.recover_remote().await.is_err());
    f.machine.remote_client = broken_proof_client;
    assert!(f.machine.recover_remote().await.is_err());
    f.machine.remote_client = crate::registry::Client::new;
    // The mocked registry still identifies the original issuer, so changing
    // the persisted platform endpoint exposes a mismatched lease on recovery.
    sql(
        &f.machine,
        &format!("UPDATE installation SET platform_origin='{}/'", f.origin),
    )
    .await;
    assert!(matches!(
        f.machine.recover_remote().await,
        Err(Error::Invalid("relay_lease_identity_mismatch"))
    ));
    sql(
        &f.machine,
        &format!("UPDATE installation SET platform_origin='{}'", f.origin),
    )
    .await;
    *f.proof.lock().expect("lock") = json!({"installation_id":"other","proof":"wrong"});
    assert!(f.machine.recover_remote().await.is_err());
    f.task.abort();
    assert!(f.machine.recover_remote().await.is_err());
    f.select(RemoteChoice::Local {}).await;
    assert!(f.machine.recover_remote().await.is_err());
    assert_eq!(
        f.worker
            .calls
            .lock()
            .expect("lock")
            .last()
            .map(String::as_str),
        Some("stop")
    );
    f.machine.close().await;
}

#[tokio::test]
async fn failed_stop_and_unreachable_existing_address_are_not_published() {
    let f = Fixture::new().await;
    f.admin().await;
    f.select(RemoteChoice::Existing {
        origin: "https://127.0.0.1:1".into(),
    })
    .await;
    assert!(f.machine.recover_remote().await.is_err());
    f.worker.fail_stop.store(true, Ordering::SeqCst);
    f.select(RemoteChoice::Local {}).await;
    assert!(f.machine.recover_remote().await.is_err());
    assert!(
        f.machine
            .remote_status()
            .await
            .expect("status")
            .public_url
            .is_none()
    );
    f.task.abort();
    f.machine.close().await;
}

#[tokio::test]
async fn local_withdrawal_stops_before_a_broken_platform_client() {
    let mut f = Fixture::new().await;
    f.admin().await;
    f.select(RemoteChoice::Local {}).await;
    f.machine.remote_client = broken_client;
    assert!(f.machine.recover_remote().await.is_err());
    assert_eq!(f.worker.calls.lock().expect("calls").as_slice(), ["stop"]);
    f.task.abort();
    f.machine.close().await;
}
