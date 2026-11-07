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
            Duration::from_millis(20),
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
