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

#[tokio::test]
async fn ipv6_listener_and_remote_recovery_failures_keep_local_runtime_independent() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    serve_until(
        machine.clone(),
        "[::1]:0".parse().expect("address"),
        dir.path().join("missing"),
        async { tokio::time::sleep(Duration::from_millis(30)).await },
    )
    .await
    .expect("shutdown");
    assert!(
        tokio::time::timeout(Duration::from_millis(30), recover_remote(machine.clone()))
            .await
            .is_err()
    );
    assert!(
        serve_until(
            machine,
            "127.0.0.1:0".parse().expect("address"),
            dir.path().join("missing"),
            async {}
        )
        .await
        .is_err()
    );
}

fn failed_address(_: &tokio::net::TcpListener) -> std::io::Result<std::net::SocketAddr> {
    Err(std::io::Error::other("address unavailable"))
}
#[tokio::test]
async fn listener_address_failure_does_not_start_background_services() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(
        serve_with_shutdown(
            machine.clone(),
            "127.0.0.1:0".parse().expect("address"),
            dir.path().join("missing"),
            Box::pin(async {}),
            failed_address
        )
        .await
        .is_err()
    );
    machine.close().await;
}

#[tokio::test]
async fn unavailable_provider_catalog_does_not_stop_node_heartbeats() {
    use axum::{Json, Router, routing::post};
    use orchestrator_protocol::{
        routing::{DispatchBatch, DispatchPoll},
        sync::{SyncRequest, SyncResponse},
    };
    let received = Arc::new(tokio::sync::Notify::new());
    let observed = received.clone();
    let app = Router::new()
        .route(
            "/api/node/enroll",
            post(|| async { Json(serde_json::json!({})) }),
        )
        .route(
            "/v1/sync",
            post(move |Json(request): Json<SyncRequest>| {
                let observed = observed.clone();
                async move {
                    observed.notify_one();
                    Json(SyncResponse {
                        version: request.version,
                        acknowledged: request.events.iter().map(|e| e.sequence).collect(),
                    })
                }
            }),
        )
        .route(
            "/v1/dispatch/poll",
            post(|Json(poll): Json<DispatchPoll>| async move {
                Json(DispatchBatch {
                    recovery: None,
                    acknowledged: poll.accepted,
                    command: None,
                })
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    machine
        .save_setting("router_origin", &origin)
        .await
        .expect("router");
    let journal = machine.agent_journal().await.expect("journal");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", journal.path().display()))
        .await
        .expect("pool");
    sqlx::query("ALTER TABLE workspace_providers RENAME COLUMN configuration TO broken")
        .execute(&pool)
        .await
        .expect("fault");
    pool.close().await;
    journal.close().await;
    let local = machine.clone();
    let worker = tokio::spawn(async move {
        run_agent(
            &local,
            std::env::current_exe().expect("executable"),
            Arc::new(AtomicBool::new(false)),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), received.notified())
        .await
        .expect("heartbeat despite catalog failure");
    worker.abort();
    let _ = worker.await;
    server.abort();
    machine.close().await;
}

#[tokio::test]
async fn agent_only_installation_does_not_run_group_schedules() {
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(crate::tests::Backend::default()))
        .await
        .expect("machine");
    let mut registration = crate::tests::registration();
    registration.role = orchestrator_protocol::installation::InstallationRole::Agent;
    machine
        .initialize(
            &registration,
            "http://127.0.0.1:8000",
            "http://127.0.0.1:8080",
        )
        .await
        .expect("initialize");
    machine
        .reconcile_automations()
        .await
        .expect("agent skips scheduling");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), schedule(machine.clone()))
            .await
            .is_err()
    );
    let app = crate::web::router(machine.clone());
    let cookie = crate::tests::http::sign_in(&app, &machine).await;
    for body in [
        None,
        Some(
            serde_json::json!({"type":"run","workflow_id":uuid::Uuid::from_u128(1),"run_id":uuid::Uuid::from_u128(2)}),
        ),
    ] {
        assert!(
            !crate::tests::http::request(
                &app,
                "/api/local/automations",
                body,
                Some(&cookie),
                Some("http://127.0.0.1:8080")
            )
            .await
            .0
            .is_success()
        );
    }
    machine.close().await;
}

#[tokio::test]
async fn missing_updater_does_not_stop_runtime_recovery() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            recover_updates_with(machine.clone(), |_| Err(Error::Invalid("unavailable")))
        )
        .await
        .is_err()
    );
    machine.close().await;
}
