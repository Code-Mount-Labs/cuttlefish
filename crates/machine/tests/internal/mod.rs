#![allow(clippy::expect_used)]
use crate::{
    Error,
    resources::{DatabaseBackend, DatabaseChoice, DatabaseSpec, ResourceFuture},
    state::Machine,
};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationRole, Registration},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
#[derive(Debug, Default)]
pub(crate) struct Backend {
    specs: Mutex<Vec<String>>,
    pub(crate) fail: AtomicBool,
    pause: AtomicBool,
    entered: tokio::sync::Notify,
}
impl DatabaseBackend for Backend {
    fn reconcile<'a>(&'a self, spec: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async move {
            self.specs
                .lock()
                .expect("lock")
                .push(serde_json::to_string(spec).expect("spec"));
            self.entered.notify_one();
            if self.pause.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Dependency("simulated_failure"));
            }
            Ok(())
        })
    }
}
pub(crate) fn registration() -> Registration {
    Registration {
        group_id: GroupId::try_from(Uuid::from_u128(1)).expect("group"),
        installation_id: InstallationId::try_from(Uuid::from_u128(2)).expect("id"),
        role: InstallationRole::RouterAgent,
        name: "Local workstation".to_owned().try_into().expect("name"),
        router_url: Some("http://127.0.0.1:8080".into()),
        credential: "a".repeat(64).try_into().expect("credential"),
        invitation: "b".repeat(64).try_into().expect("invitation"),
    }
}
pub(crate) async fn initialize(machine: &Machine) {
    machine
        .initialize(
            &registration(),
            "http://127.0.0.1:8081",
            "http://127.0.0.1:8080",
        )
        .await
        .expect("identity");
}
#[tokio::test]
async fn deferred_setup_survives_restart_and_can_be_completed_later() {
    let directory = tempfile::tempdir().expect("directory");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("open");
    initialize(&machine).await;
    initialize(&machine).await;
    assert_eq!(
        machine
            .configure(DatabaseChoice::Later)
            .await
            .expect("skip")
            .state,
        "unconfigured"
    );
    assert!(backend.specs.lock().expect("lock").is_empty());
    assert!(machine.execution_store().await.is_err());
    assert!(
        Machine::open(directory.path(), backend.clone())
            .await
            .is_err()
    );
    machine.close().await;
    drop(machine);
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("restart");
    assert_eq!(
        machine.status().await.expect("status").state,
        "unconfigured"
    );
    assert_eq!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://owner:secret@localhost/user_database".into()
            })
            .await
            .expect("configure later")
            .state,
        "ready"
    );
    assert!(
        !serde_json::to_string(&machine.status().await.expect("status"))
            .expect("json")
            .contains("secret")
    );
    assert!(machine.configure(DatabaseChoice::Later).await.is_err());
    machine.close().await;
}
#[tokio::test]
async fn interruption_after_saving_a_managed_plan_reuses_exact_credentials_and_volume() {
    let directory = tempfile::tempdir().expect("directory");
    let backend = Arc::new(Backend::default());
    backend.pause.store(true, Ordering::SeqCst);
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("open");
    initialize(&machine).await;
    let working = machine.clone();
    let job = tokio::spawn(async move {
        working
            .configure(DatabaseChoice::Managed { port: 54329 })
            .await
    });
    backend.entered.notified().await;
    assert_eq!(
        machine.status().await.expect("saved before work").state,
        "provisioning"
    );
    let original = backend.specs.lock().expect("lock")[0].clone();
    job.abort();
    assert!(job.await.expect_err("aborted").is_cancelled());
    machine.close().await;
    drop(machine);
    backend.pause.store(false, Ordering::SeqCst);
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("restart");
    assert_eq!(machine.recover().await.expect("recover").state, "ready");
    assert_eq!(backend.specs.lock().expect("lock")[1], original);
    assert_eq!(machine.status().await.expect("status").revision, 1);
    machine.close().await;
}
#[tokio::test]
async fn local_admin_can_sign_in_before_database_setup_and_after_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("open");
    initialize(&machine).await;
    let setup = machine.bootstrap_token().expect("setup");
    assert_eq!(
        machine.bootstrap_token().expect("saved").expose(),
        setup.expose()
    );
    let credentials = || crate::auth::Credentials {
        username: "operator".into(),
        password: "long-local-password".into(),
    };
    let token = machine
        .setup_admin(&setup, credentials())
        .await
        .expect("setup")
        .try_into()
        .expect("token");
    assert!(machine.setup_admin(&setup, credentials()).await.is_err());
    machine.authenticate(&token).await.expect("session");
    machine.close().await;
    drop(machine);
    let machine = Machine::open(directory.path(), backend)
        .await
        .expect("restart");
    machine
        .authenticate(&token)
        .await
        .expect("restored session");
    machine.logout(&token).await.expect("logout");
    assert!(machine.authenticate(&token).await.is_err());
    let token = machine
        .login(credentials())
        .await
        .expect("login")
        .try_into()
        .expect("token");
    machine.authenticate(&token).await.expect("signed in");
    assert_eq!(
        machine.status().await.expect("database pending").state,
        "unconfigured"
    );
    machine.close().await;
}

#[cfg(feature = "docker-tests")]
mod docker_live;

mod http;
mod registry;
mod state_cases;

pub(crate) mod end_to_end;

mod faults;
