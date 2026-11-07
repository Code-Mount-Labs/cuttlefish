#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{Backend, initialize, registration};
#[tokio::test]
async fn agent_startup_errors_leave_the_dashboard_runtime_independent() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    let ready = Arc::new(AtomicBool::new(false));
    let worker = tempfile::NamedTempFile::new().expect("worker");
    assert!(
        run_agent(&machine, worker.path().into(), ready.clone())
            .await
            .is_err()
    );
    initialize(&machine).await;
    agent(machine.clone(), dir.path().join("missing"), ready.clone()).await;
    assert!(!ready.load(Ordering::SeqCst));
    std::fs::write(dir.path().join("agent"), "not a directory").expect("block journal");
    assert!(
        run_agent(&machine, worker.path().into(), ready.clone())
            .await
            .is_err()
    );
    std::fs::remove_file(dir.path().join("agent")).expect("remove blocker");
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            run_agent(&machine, worker.path().into(), ready.clone())
        )
        .await
        .is_err()
    );
    machine
        .save_setting("router_origin", "http://127.0.0.1:1")
        .await
        .expect("unavailable router");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(40),
            run_agent(&machine, worker.path().into(), ready.clone())
        )
        .await
        .is_err()
    );
    machine
        .save_setting("router_origin", "invalid")
        .await
        .expect("bad router");
    assert!(
        run_agent(&machine, worker.path().into(), ready)
            .await
            .is_err()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("occupied");
    assert!(
        serve_until(
            machine.clone(),
            listener.local_addr().expect("address"),
            worker.path().into(),
            async {}
        )
        .await
        .is_err()
    );
    machine.close().await;
}
#[tokio::test]
async fn reporting_and_recovery_errors_do_not_end_their_loops() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            report(machine.clone(), Arc::new(AtomicBool::new(false)))
        )
        .await
        .is_err()
    );
    let mut identity = registration();
    identity.role = orchestrator_protocol::installation::InstallationRole::Router;
    machine
        .initialize(&identity, "http://127.0.0.1:1", "http://127.0.0.1:8080")
        .await
        .expect("identity");
    run_agent(
        &machine,
        dir.path().join("missing"),
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .expect("no agent requested");
    agent(
        machine.clone(),
        dir.path().join("missing"),
        Arc::new(AtomicBool::new(true)),
    )
    .await;
    sqlx::query("UPDATE resources SET state='provisioning',spec='{}'")
        .execute(&machine.pool)
        .await
        .expect("bad stored spec");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), recover(machine.clone()))
            .await
            .is_err()
    );
    machine.close().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), recover(machine))
            .await
            .is_err()
    );
}
#[test]
fn native_platform_classification_handles_supported_hosts() {
    assert_eq!(native_platform("windows"), Platform::Windows);
    assert_eq!(native_platform("linux"), Platform::Linux);
    assert_eq!(native_platform("macos"), Platform::MacOs);
}

#[tokio::test]
async fn recovery_reports_backend_failure_and_invalid_router_changes_stop_the_agent() {
    let dir = tempfile::tempdir().expect("directory");
    let backend = Arc::new(Backend::default());
    backend.fail.store(true, Ordering::SeqCst);
    let machine = Machine::open(dir.path(), backend).await.expect("machine");
    initialize(&machine).await;
    machine
        .select_database(crate::resources::DatabaseChoice::Existing {
            url: "postgresql://host/db".into(),
        })
        .await
        .expect("select");
    let recovery = tokio::spawn(recover(machine.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while machine.status().await.expect("status").state != "failed" {
            tokio::task::yield_now().await;
        }
        // The stored state can become visible before the recovery future resumes.
        // Wait for its transaction guard to be released so cancellation cannot
        // cut off error reporting immediately after the journal commit.
        drop(machine.changes.lock().await);
    })
    .await
    .expect("recovery completes despite the backend failure");
    assert!(
        !recovery.is_finished(),
        "recovery continues serving retries"
    );
    recovery.abort();
    assert!(recovery.await.expect_err("cancelled loop").is_cancelled());
    assert_eq!(machine.status().await.expect("status").state, "failed");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    machine
        .save_setting(
            "router_origin",
            &format!("http://{}", listener.local_addr().expect("address")),
        )
        .await
        .expect("router");
    let change = machine.clone();
    let app = axum::Router::new().route(
        "/api/node/enroll",
        axum::routing::post(move || {
            let machine = change.clone();
            async move {
                machine
                    .save_setting("router_origin", "invalid")
                    .await
                    .expect("change origin");
                axum::Json(serde_json::json!({"ok":true}))
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    let worker = tempfile::NamedTempFile::new().expect("worker");
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        run_agent(
            &machine,
            worker.path().into(),
            Arc::new(AtomicBool::new(false)),
        ),
    )
    .await
    .expect("stops after origin change");
    assert!(matches!(
        result,
        Err(Error::Dependency("invalid_router_configuration"))
    ));
    server.abort();
    machine.close().await;
}
