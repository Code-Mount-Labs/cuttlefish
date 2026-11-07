#![allow(clippy::expect_used)]
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct FaultProxy {
    port: u16,
    hit: Arc<AtomicBool>,
    job: tokio::task::JoinHandle<()>,
}
impl FaultProxy {
    async fn start(upstream: u16, needle: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("proxy listen");
        let port = listener.local_addr().expect("address").port();
        let hit = Arc::new(AtomicBool::new(false));
        let seen = hit.clone();
        let job = tokio::spawn(async move {
            let (mut client, _) = listener.accept().await.expect("client");
            let mut database = TcpStream::connect(("127.0.0.1", upstream))
                .await
                .expect("database");
            startup(&mut client, &mut database).await.expect("startup");
            let (mut client_read, mut client_write) = client.split();
            let (mut database_read, mut database_write) = database.split();
            let forward = async {
                loop {
                    let kind = client_read.read_u8().await?;
                    let length = client_read.read_u32().await?;
                    let mut body = vec![0; usize::try_from(length - 4).expect("length")];
                    client_read.read_exact(&mut body).await?;
                    if body
                        .windows(needle.len())
                        .any(|bytes| bytes == needle.as_bytes())
                    {
                        seen.store(true, Ordering::SeqCst);
                        return Ok::<_, std::io::Error>(());
                    }
                    database_write.write_u8(kind).await?;
                    database_write.write_u32(length).await?;
                    database_write.write_all(&body).await?;
                }
            };
            tokio::select! {
                _ = forward => {},
                _ = tokio::io::copy(&mut database_read,&mut client_write) => {},
            }
        });
        Self { port, hit, job }
    }
    async fn verify(self) {
        tokio::time::timeout(Duration::from_secs(5), self.job)
            .await
            .expect("proxy finished")
            .expect("proxy task");
        assert!(
            self.hit.load(Ordering::SeqCst),
            "requested fault was reached"
        );
    }
}
async fn startup(client: &mut TcpStream, database: &mut TcpStream) -> std::io::Result<()> {
    loop {
        let length = client.read_u32().await?;
        let mut body = vec![0; usize::try_from(length - 4).expect("startup length")];
        client.read_exact(&mut body).await?;
        database.write_u32(length).await?;
        database.write_all(&body).await?;
        if length != 8 {
            return Ok(());
        }
        let reply = database.read_u8().await?;
        client.write_u8(reply).await?;
        assert_eq!(reply, b'N', "disposable PostgreSQL must not negotiate TLS");
    }
}
fn admin_url(port: u16, password: &str) -> String {
    format!("postgresql://postgres:{password}@127.0.0.1:{port}/cuttlefish")
}
async fn connect_ready(port: u16, password: &str) -> sqlx::PgPool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(pool) = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url(port, password))
            .await
        {
            return pool;
        }
        assert!(tokio::time::Instant::now() < deadline, "PostgreSQL startup");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn role_faults(port: u16, password: &str, admin: &sqlx::PgPool) {
    for statement in [
        "BEGIN",
        "SELECT pg_advisory_xact_lock",
        "SELECT rolsuper",
        "CREATE ROLE",
        "ALTER DATABASE",
        "REVOKE CONNECT",
        "COMMIT",
    ] {
        let proxy = FaultProxy::start(port, statement).await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            prepare_application_role(proxy.port, password, password),
        )
        .await
        .expect("bounded operation");
        assert!(outcome.is_err(), "{statement}");
        proxy.verify().await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_roles WHERE rolname='cuttlefish'")
                .fetch_one(admin)
                .await
                .expect("role count");
        assert_eq!(count, 0, "failed transaction must not provision a role");
    }
    sqlx::query("CREATE ROLE cuttlefish SUPERUSER")
        .execute(admin)
        .await
        .expect("unexpected role");
    assert!(matches!(
        prepare_application_role(port, password, password).await,
        Err(Error::Invalid(
            "application_database_role_has_unexpected_privileges"
        ))
    ));
    sqlx::query("DROP ROLE cuttlefish")
        .execute(admin)
        .await
        .expect("remove bad role");
    prepare_application_role(port, password, password)
        .await
        .expect("provision role");
}
async fn probe_faults(port: u16, password: &str, admin: &sqlx::PgPool) {
    let proxy = FaultProxy::start(port, "SELECT pg_is_in_recovery").await;
    let spec = DatabaseSpec::Existing {
        url: admin_url(proxy.port, password),
    };
    assert!(PostgresProbe.verify(&spec).await.is_err());
    proxy.verify().await;
    let unavailable = TcpListener::bind("127.0.0.1:0").await.expect("unused port");
    let closed = unavailable.local_addr().expect("port").port();
    drop(unavailable);
    assert!(
        PostgresProbe
            .verify(&DatabaseSpec::Existing {
                url: admin_url(closed, password)
            })
            .await
            .is_err()
    );
    let backend = DockerBackend::new(std::path::Path::new("."));
    assert!(
        backend
            .reconcile(&DatabaseSpec::Existing {
                url: admin_url(closed, password)
            })
            .await
            .is_err()
    );
    sqlx::query("CREATE TABLE _sqlx_migrations(invalid TEXT)")
        .execute(admin)
        .await
        .expect("bad migration journal");
    assert!(matches!(
        PostgresProbe
            .verify(&DatabaseSpec::Existing {
                url: admin_url(port, password)
            })
            .await,
        Err(Error::Dependency("execution_schema_unavailable"))
    ));
}
#[tokio::test]
async fn transaction_and_probe_disconnects_never_activate_an_unverified_database() {
    let dir = tempfile::tempdir().expect("directory");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("port");
    let port = listener.local_addr().expect("port").port();
    drop(listener);
    let name = format!("cuttlefish-pg-{}", uuid::Uuid::new_v4().simple());
    let owned_name = name.clone();
    let path = dir.path().to_owned();
    let job = tokio::spawn(async move {
        let password = "a".repeat(64);
        let backend = DockerBackend::new(&path);
        backend
            .ensure_managed(&owned_name, &owned_name, port, &password)
            .await
            .expect("container");
        let admin = connect_ready(port, &password).await;
        role_faults(port, &password, &admin).await;
        probe_faults(port, &password, &admin).await;
        admin.close().await;
    });
    let result = job.await;
    for args in [vec!["rm", "--force", &name], vec!["volume", "rm", &name]] {
        assert!(
            Command::new(docker_binary())
                .args(args)
                .output()
                .await
                .expect("cleanup")
                .status
                .success()
        );
    }
    result.expect("fault scenarios");
}
