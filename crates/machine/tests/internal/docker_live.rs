use super::*;
use crate::docker::DockerBackend;
use std::time::Duration;

#[tokio::test]
async fn real_postgres_volume_and_application_role_survive_setup_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let port = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("port")
        .local_addr()
        .expect("address")
        .port();
    let backend = Arc::new(DockerBackend::new(directory.path()));
    let machine = Machine::open(directory.path(), backend.clone())
        .await
        .expect("open");
    initialize(&machine).await;
    let configured = machine.configure(DatabaseChoice::Managed { port }).await;
    let encoded: String = sqlx::query_scalar("SELECT spec FROM resources")
        .fetch_one(&machine.pool)
        .await
        .expect("persisted spec");
    let spec: DatabaseSpec = serde_json::from_str(&encoded).expect("spec");
    let name = match &spec {
        DatabaseSpec::Managed { container, .. } => container.clone(),
        DatabaseSpec::Existing { .. } => unreachable!(),
    };
    let checked = async {
        configured?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&spec.connection_url())
            .await?;
        let superuser: bool =
            sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname=current_user")
                .fetch_one(&pool)
                .await?;
        if superuser {
            return Err(Error::Invalid("unexpected superuser"));
        }
        sqlx::raw_sql(
            "CREATE TABLE recovery_marker(value INTEGER); INSERT INTO recovery_marker VALUES(42)",
        )
        .execute(&pool)
        .await?;
        pool.close().await;
        machine.close().await;
        drop(machine);
        let restarted = tokio::process::Command::new("docker")
            .args(["restart", &name])
            .output()
            .await?;
        if !restarted.status.success() {
            return Err(Error::Dependency("restart_failed"));
        }
        let machine = Machine::open(directory.path(), backend).await?;
        machine.recover().await?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&spec.connection_url())
            .await?;
        let marker: i32 = sqlx::query_scalar("SELECT value FROM recovery_marker")
            .fetch_one(&pool)
            .await?;
        if marker != 42 {
            return Err(Error::Invalid("lost persistent data"));
        }
        pool.close().await;
        machine.close().await;
        Ok::<(), Error>(())
    }
    .await;
    let removed = tokio::process::Command::new("docker")
        .args(["rm", "--force", &name])
        .output()
        .await
        .expect("remove test container");
    let volume = tokio::process::Command::new("docker")
        .args(["volume", "rm", &name])
        .output()
        .await
        .expect("remove test volume");
    assert!(
        removed.status.success() && volume.status.success(),
        "test cleanup failed"
    );
    checked.expect("real database recovery");
}
