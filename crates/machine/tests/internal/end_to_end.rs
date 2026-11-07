use super::*;
use crate::{
    docker::DockerBackend,
    registry::{Client, Enrollment},
};
use cuttlefish_platform::{Credentials, Platform};
use orchestrator_protocol::{
    capabilities::{Capability, Platform as Os},
    ids::NodeId,
    sync::{NodeProfile, NodeToken},
};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
pub(crate) struct Database {
    admin: PgPool,
    schema: String,
    pub(crate) url: String,
}
impl Database {
    pub(crate) async fn new() -> Self {
        let base =
            std::env::var("COORDINATOR_TEST_DATABASE_URL").expect("dedicated PostgreSQL required");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&base)
            .await
            .expect("admin");
        let schema = format!("machine_test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        Self {
            admin,
            schema,
            url: url.to_string(),
        }
    }
    pub(crate) async fn close(self) {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("cleanup");
        self.admin.close().await;
    }
}
struct Server {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(app: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server");
        });
        Self { origin, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn enrolled_machine(
    platform: &Platform,
    origin: &str,
    directory: &std::path::Path,
    public_origin: &str,
) -> Machine {
    let account: NodeToken = platform
        .register_account(Credentials {
            username: "operator".into(),
            password: "long-enough-password".into(),
        })
        .await
        .expect("account")
        .try_into()
        .expect("token");
    let group = platform
        .create_group(&account, "Test group")
        .await
        .expect("group")
        .try_into()
        .expect("id");
    let mut registration = registration();
    registration.group_id = group;
    registration.router_url = Some(public_origin.into());
    registration.invitation = platform
        .invite(&account, group)
        .await
        .expect("invitation")
        .try_into()
        .expect("token");
    let machine = Machine::open(directory, Arc::new(DockerBackend::new(directory)))
        .await
        .expect("machine");
    machine
        .initialize(&registration, origin, public_origin)
        .await
        .expect("identity");
    machine.register().await.expect("registered");
    machine
}
async fn cycle(machine: &Machine, server: &Server, directory: &std::path::Path) {
    use orchestrator_node::{
        model::NodeIdentity, process::LocalLauncher, runtime::NodeRuntime, sync::Remote,
    };
    let registration = machine.identity().await.expect("identity").registration;
    let store = machine.execution_store().await.expect("store");
    let mut node = NodeRuntime::open(
        directory,
        NodeIdentity {
            group_id: registration.group_id,
            node_id: NodeId::from(registration.installation_id),
        },
        LocalLauncher::new(std::env::current_exe().expect("executable")),
    )
    .await
    .expect("node journal");
    let remote = Remote::new(&server.origin, &registration.credential).expect("remote");
    remote
        .cycle(
            &mut node,
            &NodeProfile {
                platform: Os::Linux,
                capabilities: Capability::native(),
                capacity: 4,
            },
        )
        .await
        .expect("sync through installed router");
    let nodes = store.nodes(&registration.credential).await.expect("nodes");
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].profile.is_some());
}
#[tokio::test]
async fn hosted_directory_and_user_database_are_separate_and_enrollment_survives_restart() {
    let registry_db = Database::new().await;
    let execution_db = Database::new().await;
    let platform = Platform::open(&registry_db.url, "http://127.0.0.1:8080")
        .await
        .expect("platform");
    let directory_server = Server::start(cuttlefish_platform::web::router(platform.clone())).await;
    let directory = tempfile::tempdir().expect("dir");
    let machine = enrolled_machine(
        &platform,
        &directory_server.origin,
        directory.path(),
        "http://127.0.0.1:8080",
    )
    .await;
    machine
        .configure(DatabaseChoice::Existing {
            url: execution_db.url.clone(),
        })
        .await
        .expect("user database");
    let server = Server::start(crate::web::router(machine.clone())).await;
    let registration = machine.identity().await.expect("identity").registration;
    let client = Client::new(&server.origin).expect("client");
    let enrollment = Enrollment::from(&registration);
    client
        .post::<_, Value>("/api/node/enroll", None, &enrollment)
        .await
        .expect("enrolled");
    verify_transport_identity(&machine, &registration).await;
    let journal = tempfile::tempdir().expect("journal");
    cycle(&machine, &server, journal.path()).await;
    machine.advertise(true).await.expect("ready");
    verify_directory_count(&registry_db).await;
    let wrong = json!({"installation_id":registration.installation_id,"credential":"c".repeat(64)});
    assert!(
        client
            .post::<_, Value>("/api/node/enroll", None, &wrong)
            .await
            .is_err()
    );
    directory_server.task.abort();
    platform.close().await;
    client
        .post::<_, Value>("/api/node/enroll", None, &enrollment)
        .await
        .expect("known agent during directory outage");
    drop(server);
    tokio::task::yield_now().await;
    let store = machine.execution_store().await.expect("store");
    store.close().await;
    drop(store);
    machine.close().await;
    drop(machine);
    let machine = Machine::open(
        directory.path(),
        Arc::new(DockerBackend::new(directory.path())),
    )
    .await
    .expect("restart");
    machine.recover().await.expect("recheck primary");
    let server = Server::start(crate::web::router(machine.clone())).await;
    Client::new(&server.origin)
        .expect("client")
        .post::<_, Value>("/api/node/enroll", None, &enrollment)
        .await
        .expect("enrollment persisted");
    cycle(&machine, &server, journal.path()).await;
    machine
        .execution_store()
        .await
        .expect("store")
        .revoke(
            registration.group_id,
            NodeId::from(registration.installation_id),
        )
        .await
        .expect("revoke");
    assert!(
        Client::new(&server.origin)
            .expect("client")
            .post::<_, Value>("/api/node/enroll", None, &enrollment)
            .await
            .is_err()
    );
    drop(server);
    machine
        .execution_store()
        .await
        .expect("store")
        .close()
        .await;
    machine.close().await;
    registry_db.close().await;
    execution_db.close().await;
}

async fn verify_directory_count(registry_db: &Database) {
    let (resources,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {}.installations",
        registry_db.schema
    )))
    .fetch_one(&registry_db.admin)
    .await
    .expect("directory");
    assert_eq!(resources, 1);
}

#[tokio::test]
async fn service_recovers_database_and_resumes_the_same_agent_on_restart() {
    let registry_db = Database::new().await;
    let execution_db = Database::new().await;
    let platform = Platform::open(&registry_db.url, "http://127.0.0.1:8080")
        .await
        .expect("platform");
    let directory_server = Server::start(cuttlefish_platform::web::router(platform.clone())).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
    let address = listener.local_addr().expect("address");
    drop(listener);
    let directory = tempfile::tempdir().expect("directory");
    let origin = format!("http://{address}");
    let machine = enrolled_machine(
        &platform,
        &directory_server.origin,
        directory.path(),
        &origin,
    )
    .await;
    machine
        .select_database(DatabaseChoice::Existing {
            url: execution_db.url.clone(),
        })
        .await
        .expect("database");
    let credential = machine
        .identity()
        .await
        .expect("identity")
        .registration
        .credential;
    run_service_to_synced(machine.clone(), address, &credential).await;
    drop(machine);
    let raw = PgPoolOptions::new()
        .max_connections(1)
        .connect(&execution_db.url)
        .await
        .expect("raw");
    sqlx::query("UPDATE nodes SET profile=NULL")
        .execute(&raw)
        .await
        .expect("clear presence");
    raw.close().await;
    let restarted = Machine::open(
        directory.path(),
        Arc::new(DockerBackend::new(directory.path())),
    )
    .await
    .expect("restart");
    assert_eq!(
        restarted.status().await.expect("status").state,
        "provisioning"
    );
    run_service_to_synced(restarted, address, &credential).await;
    platform.close().await;
    drop(directory_server);
    registry_db.close().await;
    execution_db.close().await;
}
async fn run_service_to_synced(
    machine: Machine,
    address: std::net::SocketAddr,
    credential: &NodeToken,
) {
    let url = machine.pool.clone();
    let (stop, done) = tokio::sync::oneshot::channel();
    let local = machine.clone();
    let job = tokio::spawn(async move {
        crate::runtime::serve_until(
            local,
            address,
            std::env::current_exe().expect("worker path"),
            async {
                let _ = done.await;
            },
        )
        .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Ok(store) = machine.execution_store().await
            && store
                .nodes(credential)
                .await
                .is_ok_and(|nodes| nodes.len() == 1 && nodes[0].profile.is_some())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent did not synchronize"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let store = machine.execution_store().await.expect("store");
    stop.send(()).expect("stop");
    job.await.expect("service task").expect("service exit");
    assert!(url.is_closed());
    store.close().await;
}

async fn verify_transport_identity(machine: &Machine, registration: &Registration) {
    assert_eq!(
        machine
            .execution_store()
            .await
            .expect("store")
            .identity(&registration.credential)
            .await
            .expect("identity"),
        (
            registration.group_id,
            NodeId::from(registration.installation_id)
        )
    );
}

#[tokio::test]
async fn execution_primary_rejects_a_read_only_database() {
    use crate::docker::{DatabaseProbe, PostgresProbe};
    let base = std::env::var("COORDINATOR_TEST_DATABASE_URL").expect("test database");
    let mut url = reqwest::Url::parse(&base).expect("URL");
    url.query_pairs_mut()
        .append_pair("options", "-cdefault_transaction_read_only=on");
    let result = PostgresProbe
        .verify(&DatabaseSpec::Existing {
            url: url.to_string(),
        })
        .await;
    assert!(matches!(
        result,
        Err(Error::Invalid("primary_database_is_read_only"))
    ));
}
