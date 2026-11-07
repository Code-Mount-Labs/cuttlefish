use crate::tests::{
    common::Server,
    data,
    delivery::{DeliveryLauncher, runtime},
    routing::id,
};
use axum::{Json, Router, http::StatusCode, routing::post};
use orchestrator_node::{runtime::NodeRuntime, sync::Remote};
use orchestrator_protocol::{
    routing::{DispatchBatch, DispatchPoll},
    sync::{SyncRequest, SyncResponse},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

async fn raw(node: &NodeRuntime<DeliveryLauncher>) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(node.journal().path())
            .foreign_keys(true),
    )
    .await
    .expect("raw journal")
}

#[tokio::test]
async fn delivery_retries_do_not_spawn_twice_and_invalid_responses_keep_receipts() {
    let (_temp, mut node, launcher) = runtime().await;
    let server = Server::start(Router::new().route(
        "/v1/dispatch/poll",
        post(|Json(poll): Json<DispatchPoll>| async move {
            Json(DispatchBatch {
                acknowledged: poll.accepted,
                command: Some(data::report().events[0].command.clone()),
            })
        }),
    ))
    .await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    remote.dispatch(&mut node).await.expect("first delivery");
    remote.dispatch(&mut node).await.expect("repeated delivery");
    assert_eq!(launcher.launches.lock().expect("launches").len(), 1);
    assert_eq!(
        node.journal()
            .dispatch_poll()
            .await
            .expect("receipts")
            .accepted,
        vec![id(6)]
    );
    server.close().await;
    assert!(remote.dispatch(&mut node).await.is_err());
    for (status, body) in [
        (StatusCode::UNAUTHORIZED, "denied".into()),
        (StatusCode::FOUND, "redirect".into()),
        (StatusCode::OK, "x".repeat(1_048_577)),
        (StatusCode::OK, "broken json".into()),
        (
            StatusCode::OK,
            r#"{"acknowledged":[],"command":null}"#.into(),
        ),
    ] {
        let server = Server::start(Router::new().route(
            "/v1/dispatch/poll",
            post(move || {
                let body = body.clone();
                async move { (status, body) }
            }),
        ))
        .await;
        assert!(
            Remote::new(&server.origin, &data::token())
                .expect("remote")
                .dispatch(&mut node)
                .await
                .is_err()
        );
        assert_eq!(
            node.journal()
                .dispatch_poll()
                .await
                .expect("receipts")
                .accepted,
            vec![id(6)]
        );
        server.close().await;
    }
    node.journal().close().await;
    assert!(remote.dispatch(&mut node).await.is_err());
}

#[tokio::test]
async fn acceptance_and_acknowledgment_storage_failures_are_recoverable() {
    for fault in [
        "CREATE TRIGGER reject BEFORE INSERT ON dispatch_receipts BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER reject BEFORE DELETE ON dispatch_receipts BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TRIGGER reject BEFORE INSERT ON command_receipts BEGIN SELECT RAISE(ABORT,'injected'); END",
    ] {
        let (_temp, mut node, launcher) = runtime().await;
        let pool = raw(&node).await;
        if fault.contains("BEFORE DELETE") {
            node.submit(&data::report().events[0].command)
                .await
                .expect("submit");
            node.journal()
                .received_dispatch(id(6))
                .await
                .expect("receipt");
        }
        sqlx::raw_sql(fault).execute(&pool).await.expect("fault");
        let server = Server::start(Router::new().route(
            "/v1/dispatch/poll",
            post(|Json(poll): Json<DispatchPoll>| async move {
                Json(DispatchBatch {
                    acknowledged: poll.accepted,
                    command: Some(data::report().events[0].command.clone()),
                })
            }),
        ))
        .await;
        let remote = Remote::new(&server.origin, &data::token()).expect("remote");
        assert!(remote.dispatch(&mut node).await.is_err());
        sqlx::query("DROP TRIGGER reject")
            .execute(&pool)
            .await
            .expect("repair");
        remote.dispatch(&mut node).await.expect("retry");
        assert_eq!(launcher.launches.lock().expect("launches").len(), 1);
        pool.close().await;
        node.journal().close().await;
        server.close().await;
    }
}

#[tokio::test]
async fn journal_dispatch_snapshot_and_atomic_ack_propagate_storage_failures() {
    for fault in [
        "ALTER TABLE workspace_mounts RENAME TO hidden",
        "ALTER TABLE dispatch_receipts RENAME TO hidden",
        "ALTER TABLE local_sessions RENAME TO hidden",
    ] {
        let (_temp, node, _) = runtime().await;
        let pool = raw(&node).await;
        sqlx::raw_sql(fault).execute(&pool).await.expect("fault");
        assert!(node.journal().dispatch_poll().await.is_err());
        pool.close().await;
        node.journal().close().await;
    }
    let (_temp, mut node, _) = runtime().await;
    let pool = raw(&node).await;
    assert!(node.journal().received_dispatch(id(6)).await.is_err());
    node.submit(&data::report().events[0].command)
        .await
        .expect("accepted");
    node.journal()
        .received_dispatch(id(6))
        .await
        .expect("receipt");
    sqlx::raw_sql("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(parent INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER reject AFTER DELETE ON dispatch_receipts BEGIN INSERT INTO child VALUES(42); END;").execute(&pool).await.expect("deferred failure");
    assert!(node.journal().acknowledge_dispatch(&[id(6)]).await.is_err());
    assert_eq!(
        node.journal()
            .dispatch_poll()
            .await
            .expect("rollback")
            .accepted,
        vec![id(6)]
    );
    pool.close().await;
    node.journal().close().await;
    assert!(node.journal().acknowledge_dispatch(&[id(6)]).await.is_err());
}

#[tokio::test]
async fn execution_loop_retries_and_shutdown_preserves_workers() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server = Server::start(
        Router::new()
            .route(
                "/v1/sync",
                post(|Json(report): Json<SyncRequest>| async move {
                    Json(SyncResponse {
                        version: report.version,
                        acknowledged: vec![],
                    })
                }),
            )
            .route(
                "/v1/dispatch/poll",
                post(move |Json(poll): Json<DispatchPoll>| {
                    let count = observed.fetch_add(1, Ordering::SeqCst);
                    async move {
                        use axum::response::IntoResponse;
                        if count == 0 {
                            StatusCode::SERVICE_UNAVAILABLE.into_response()
                        } else {
                            Json(DispatchBatch {
                                acknowledged: poll.accepted,
                                command: Some(data::report().events[0].command.clone()),
                            })
                            .into_response()
                        }
                    }
                }),
            ),
    )
    .await;
    let (_temp, mut node, launcher) = runtime().await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let shutdown: Pin<Box<dyn Future<Output = std::io::Result<()>>>> = Box::pin(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        })
        .await
        .expect("retry");
        Ok(())
    });
    remote
        .execute(&mut node, &data::report().profile, shutdown)
        .await
        .expect("shutdown");
    assert_eq!(launcher.launches.lock().expect("launches").len(), 1);
    let shutdown: Pin<Box<dyn Future<Output = std::io::Result<()>>>> =
        Box::pin(async { Err(std::io::Error::other("signal failure")) });
    assert!(
        remote
            .execute(&mut node, &data::report().profile, shutdown)
            .await
            .is_err()
    );
    server.close().await;
    assert!(
        remote
            .cycle(&mut node, &data::report().profile)
            .await
            .is_err()
    );
    node.journal().close().await;
    assert!(
        remote
            .cycle(&mut node, &data::report().profile)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn truncated_delivery_and_model_responses_fail_closed() {
    for model in [false, true] {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut bytes = vec![0; 65536];
            assert!(stream.read(&mut bytes).await.expect("request") > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
                .await
                .expect("write");
            stream.flush().await.expect("flush");
            tokio::time::sleep(Duration::from_millis(50)).await;
        });
        if model {
            use orchestrator_coordinator::selector::{LlmSelector, Selector};
            use orchestrator_protocol::routing::{Candidate, RoutingContext};
            let context = RoutingContext {
                active_work: vec![],
                request: crate::tests::routing::request(100),
                candidates: vec![Candidate {
                    node_id: id(2),
                    profile: data::report().profile,
                    available_slots: 1,
                }],
                recent_activity: vec![],
            };
            assert!(
                LlmSelector::new(&origin, "key", "model")
                    .expect("model")
                    .choose(&context)
                    .await
                    .is_err()
            );
        } else {
            let (_temp, mut node, _) = runtime().await;
            assert!(
                Remote::new(&origin, &data::token())
                    .expect("remote")
                    .dispatch(&mut node)
                    .await
                    .is_err()
            );
            node.journal().close().await;
        }
        server.await.expect("join");
    }
}

#[tokio::test]
async fn failed_input_staging_and_report_reads_do_not_accept_or_acknowledge_work() {
    let (_temp, mut node, launcher) = runtime().await;
    let server = Server::start(Router::new().route(
        "/v1/dispatch/poll",
        post(|Json(poll): Json<DispatchPoll>| async move {
            let mut command = data::report().events[0].command.clone();
            if let orchestrator_protocol::agent::AgentAction::Start { message, .. } =
                &mut command.action
            {
                message
                    .attachments
                    .push(orchestrator_protocol::attachments::Attachment {
                        id: orchestrator_protocol::ids::AttachmentId::random(),
                        workspace_id: id(5),
                        name: "missing.txt".into(),
                        media_type: "text/plain".into(),
                        size: 0,
                        sha256: "a".repeat(64),
                    });
            }
            Json(DispatchBatch {
                acknowledged: poll.accepted,
                command: Some(command),
            })
        }),
    ))
    .await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    assert!(remote.dispatch(&mut node).await.is_err());
    assert!(launcher.launches.lock().expect("launches").is_empty());
    let pool = raw(&node).await;
    sqlx::query("ALTER TABLE local_sessions RENAME COLUMN state TO missing_state")
        .execute(&pool)
        .await
        .expect("fault");
    assert!(
        node.journal()
            .sync_report(data::report().profile.clone())
            .await
            .is_err()
    );
    assert!(
        remote
            .synchronize(node.journal(), &data::report().profile)
            .await
            .is_err()
    );
    pool.close().await;
    node.journal().close().await;
    server.close().await;
}
