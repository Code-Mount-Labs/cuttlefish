//! Smoke tests for the actual platform executable and its shutdown path.
#![allow(clippy::expect_used)]
#[cfg(unix)]
use sqlx::postgres::PgPoolOptions;
#[cfg(unix)]
use std::{process::Stdio, time::Duration};
use tokio::process::Command;
#[cfg(unix)]
use uuid::Uuid;
const BINARY: &str = env!("CARGO_BIN_EXE_cuttlefish-platform");

#[tokio::test]
async fn startup_rejects_invalid_configuration() {
    let result = Command::new(BINARY)
        .args([
            "--database-url",
            "invalid",
            "--public-origin",
            "https://platform.test",
        ])
        .output()
        .await
        .expect("process");
    assert!(!result.status.success());
}
#[cfg(unix)]
#[tokio::test]
async fn deployed_process_serves_health_and_shuts_down_gracefully() {
    let base = std::env::var("COORDINATOR_TEST_DATABASE_URL").expect("database");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base)
        .await
        .expect("connect");
    let schema = format!("platform_cli_{}", Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("URL");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let port = listener.local_addr().expect("address").port().to_string();
    let args = [
        "--database-url",
        url.as_str(),
        "--public-origin",
        "http://127.0.0.1:8080",
        "--port",
        &port,
    ];
    assert!(
        !Command::new(BINARY)
            .args(args)
            .output()
            .await
            .expect("occupied port")
            .status
            .success()
    );
    drop(listener);
    let mut child = Command::new(BINARY)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn");
    let client = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if client
                .get(format!("http://127.0.0.1:{port}/healthz"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("ready");
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().expect("pid").to_string()])
            .status()
            .await
            .expect("signal")
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("shutdown")
            .expect("status")
            .success()
    );
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
