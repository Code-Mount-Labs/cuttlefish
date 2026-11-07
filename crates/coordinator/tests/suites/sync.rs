use crate::tests::{
    common::{Fixture, Server},
    data,
};
use axum::{Json, Router, http::StatusCode, routing::post};
use orchestrator_coordinator::web;
use orchestrator_node::{journal::Journal, model::NodeIdentity, sync::Remote};
use orchestrator_protocol::sync::{NodeToken, SyncRequest, SyncResponse};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

async fn local() -> (tempfile::TempDir, Journal) {
    let temp = tempfile::tempdir().expect("local directory");
    let report = data::report();
    let command = &report.events[0].command;
    let journal = Journal::open(
        &temp.path().join("node.sqlite"),
        NodeIdentity {
            group_id: report.group_id,
            node_id: report.node_id,
        },
    )
    .await
    .expect("journal");
    let orchestrator_protocol::agent::AgentAction::Start { workspace_id, .. } = command.action
    else {
        unreachable!()
    };
    journal
        .mount(workspace_id, temp.path())
        .await
        .expect("mount");
    let token = uuid::Uuid::new_v4();
    journal.accept(command, token).await.expect("accept");
    journal
        .register(command.target.session_id, &token.to_string(), 123)
        .await
        .expect("register");
    journal
        .claim(command.target.session_id, &token.to_string())
        .await
        .expect("claim");
    journal
        .complete(
            command.command_id,
            &token.to_string(),
            &report.events[0].outcome,
        )
        .await
        .expect("complete");
    (temp, journal)
}

#[tokio::test]
async fn lost_response_and_replica_replacement_do_not_lose_or_duplicate_events() {
    let f = Fixture::enrolled().await;
    let store = f.store.clone();
    let failing = Server::start(Router::new().route(
        "/v1/sync",
        post(move |Json(report): Json<SyncRequest>| {
            let store = store.clone();
            async move {
                store
                    .sync(&data::token(), &report)
                    .await
                    .expect("commit before lost response");
                StatusCode::SERVICE_UNAVAILABLE
            }
        }),
    ))
    .await;
    let (_temp, journal) = local().await;
    let profile = data::report().profile;
    let remote = Remote::new(&failing.origin, &data::token()).expect("remote");
    assert!(remote.synchronize(&journal, &profile).await.is_err());
    assert_eq!(journal.pending_events().await.expect("outbox").len(), 1);
    assert_eq!(
        f.store
            .events(&data::token(), 0)
            .await
            .expect("committed")
            .len(),
        1
    );
    failing.close().await;
    let replacement = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("replacement replica");
    let server = Server::start(web::router(replacement.clone())).await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    assert_eq!(
        remote
            .synchronize(&journal, &profile)
            .await
            .expect("redelivery"),
        1
    );
    assert!(journal.pending_events().await.expect("acked").is_empty());
    assert_eq!(
        remote
            .synchronize(&journal, &profile)
            .await
            .expect("heartbeat only"),
        0
    );
    assert_eq!(
        replacement
            .events(&data::token(), 0)
            .await
            .expect("deduped")
            .len(),
        1
    );
    assert_eq!(
        replacement.nodes(&data::token()).await.expect("presence")[0].active_sessions,
        1
    );
    assert!(!format!("{remote:?}").contains(data::token().expose()));
    server.close().await;
    replacement.close().await;
    journal.close().await;
    f.close().await;
}

#[tokio::test]
async fn client_keeps_outbox_for_invalid_responses_redirects_and_authentication_failures() {
    for (status, body) in [
        (StatusCode::UNAUTHORIZED, "denied".into()),
        (
            StatusCode::TEMPORARY_REDIRECT,
            "{\"version\":1,\"acknowledged\":[1]}".into(),
        ),
        (StatusCode::OK, "not JSON".into()),
        (StatusCode::OK, "x".repeat(4097)),
        (
            StatusCode::OK,
            "{\"version\":1,\"acknowledged\":[999]}".into(),
        ),
        (
            StatusCode::OK,
            "{\"version\":2,\"acknowledged\":[1]}".into(),
        ),
    ] {
        let server = Server::start(Router::new().route(
            "/v1/sync",
            post(move || {
                let body = body.clone();
                async move { (status, [("location", "http://127.0.0.1:1/steal")], body) }
            }),
        ))
        .await;
        let (_temp, journal) = local().await;
        let remote = Remote::new(&server.origin, &data::token()).expect("remote");
        assert!(
            remote
                .synchronize(&journal, &data::report().profile)
                .await
                .is_err()
        );
        assert_eq!(journal.pending_events().await.expect("preserved").len(), 1);
        journal.close().await;
        server.close().await;
    }
    let (_temp, journal) = local().await;
    let remote = Remote::new("http://127.0.0.1:1", &data::token()).expect("remote");
    assert!(
        remote
            .synchronize(&journal, &data::report().profile)
            .await
            .is_err()
    );
    let mut profile = data::report().profile;
    profile.capacity = 0;
    assert!(remote.synchronize(&journal, &profile).await.is_err());
    journal.close().await;
    assert!(
        remote
            .synchronize(&journal, &data::report().profile)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn watcher_retries_and_shutdown_does_not_stop_specialists() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    let server = Server::start(Router::new().route(
        "/v1/sync",
        post(move |Json(report): Json<SyncRequest>| {
            let count = observed.fetch_add(1, Ordering::SeqCst);
            async move {
                use axum::response::IntoResponse;
                if count == 0 {
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                } else {
                    Json(SyncResponse {
                        version: report.version,
                        acknowledged: report.events.iter().map(|e| e.sequence).collect(),
                    })
                    .into_response()
                }
            }
        }),
    ))
    .await;
    let (_temp, journal) = local().await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>>>> =
        Box::pin(async {
            tokio::time::timeout(Duration::from_secs(12), async {
                while requests.load(Ordering::SeqCst) < 2 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
            .await
            .expect("retry deadline");
            Ok(())
        });
    remote
        .follow(&journal, &data::report().profile, shutdown)
        .await
        .expect("shutdown");
    assert!(
        journal
            .pending_events()
            .await
            .expect("delivered after retry")
            .is_empty()
    );
    let shutdown: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>>>> =
        Box::pin(async { Err(std::io::Error::other("signal failed")) });
    assert!(
        remote
            .follow(&journal, &data::report().profile, shutdown)
            .await
            .is_err()
    );
    let target = data::report().events[0].command.target;
    assert_eq!(
        journal
            .session(target.session_id)
            .await
            .expect("session")
            .expect("exists")
            .state,
        orchestrator_node::model::SessionStatus::Running
    );
    journal.close().await;
    server.close().await;
}

#[test]
fn endpoint_validation_rejects_insecure_or_ambiguous_origins() {
    let token: NodeToken = data::token();
    for origin in [
        "not a URL",
        "http://example.com",
        "ftp://127.0.0.1",
        "https://user:secret@example.com",
        "https://example.com/prefix",
        "https://example.com?query=1",
        "https://example.com#fragment",
    ] {
        assert!(Remote::new(origin, &token).is_err(), "{origin}");
    }
    for origin in ["https://example.com", "http://127.0.0.1", "http://[::1]"] {
        assert!(Remote::new(origin, &token).is_ok());
    }
}

#[tokio::test]
async fn journal_failures_after_remote_commit_are_safe_to_retry() {
    for delete_event in [false, true] {
        let (_temp, journal) = local().await;
        let raw = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(journal.path()),
        )
        .await
        .expect("raw journal");
        if !delete_event {
            sqlx::query("CREATE TRIGGER refuse_ack BEFORE UPDATE ON pending_events BEGIN SELECT RAISE(ABORT,'ack failed'); END").execute(&raw).await.expect("failure injection");
        }
        let handler_pool = raw.clone();
        let server = Server::start(Router::new().route(
            "/v1/sync",
            post(move |Json(report): Json<SyncRequest>| {
                let pool = handler_pool.clone();
                async move {
                    if delete_event {
                        sqlx::query("DELETE FROM pending_events")
                            .execute(&pool)
                            .await
                            .expect("concurrent deletion");
                    }
                    Json(SyncResponse {
                        version: report.version,
                        acknowledged: report.events.iter().map(|e| e.sequence).collect(),
                    })
                }
            }),
        ))
        .await;
        let remote = Remote::new(&server.origin, &data::token()).expect("remote");
        assert!(
            remote
                .synchronize(&journal, &data::report().profile)
                .await
                .is_err()
        );
        if !delete_event {
            assert_eq!(
                journal
                    .pending_events()
                    .await
                    .expect("unacknowledged")
                    .len(),
                1
            );
        }
        server.close().await;
        raw.close().await;
        journal.close().await;
    }
    let (_temp, journal) = local().await;
    let raw = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(journal.path()),
    )
    .await
    .expect("raw journal");
    sqlx::query("ALTER TABLE local_sessions RENAME TO hidden_sessions")
        .execute(&raw)
        .await
        .expect("missing session table");
    assert!(journal.sync_report(data::report().profile).await.is_err());
    raw.close().await;
    journal.close().await;
}

#[tokio::test]
async fn a_truncated_http_response_never_acknowledges_events() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0; 8192];
        assert!(socket.read(&mut buffer).await.expect("request") > 0);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{").await.expect("partial response");
        socket.flush().await.expect("flush");
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    let (_temp, journal) = local().await;
    let remote = Remote::new(&origin, &data::token()).expect("remote");
    assert!(
        remote
            .synchronize(&journal, &data::report().profile)
            .await
            .is_err()
    );
    assert_eq!(journal.pending_events().await.expect("preserved").len(), 1);
    server.await.expect("server exited");
    journal.close().await;
}
