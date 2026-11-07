#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, instance};
use axum::{Json, Router, routing::post};
use serde_json::json;
async fn running() -> Fixture {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    f
}
#[tokio::test]
async fn activity_is_fenced_deduplicated_bounded_and_recovered_after_restart() {
    let f = running().await;
    assert!(
        f.journal
            .record_activity(id(6), "stale", LocalProvider::Codex, ActivityPhase::Working)
            .await
            .is_err()
    );
    for phase in [
        ActivityPhase::Preparing,
        ActivityPhase::Preparing,
        ActivityPhase::Working,
    ] {
        f.journal
            .record_activity(id(6), &instance().to_string(), LocalProvider::Codex, phase)
            .await
            .expect("observe");
    }
    let reopened = Journal::attach(f.journal.path()).await.expect("restart");
    let saved = reopened.pending_activity().await.expect("recovered");
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].phase, ActivityPhase::Preparing);
    for i in 0..300 {
        reopened
            .record_activity(
                id(6),
                &instance().to_string(),
                LocalProvider::Codex,
                if i % 2 == 0 {
                    ActivityPhase::Waiting
                } else {
                    ActivityPhase::Working
                },
            )
            .await
            .expect("bounded");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_activity")
        .fetch_one(&reopened.pool)
        .await
        .expect("count");
    assert_eq!(count, 256);
    reopened.expire(0).await.expect("worker lost");
    let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/activity",
                post(move |Json(events): Json<Vec<ExecutionActivity>>| {
                    let observed = observed.clone();
                    async move {
                        let ids: Vec<_> = events.iter().map(|e| e.sequence).collect();
                        observed.lock().expect("lock").extend(events);
                        Json(ids)
                    }
                }),
            ),
        )
        .await
        .expect("serve");
    });
    let remote = crate::sync::Remote::new(&origin, &"a".repeat(64).try_into().expect("token"))
        .expect("remote");
    for _ in 0..6 {
        remote.upload_activity(&reopened).await.expect("sync");
    }
    assert!(reopened.pending_activity().await.expect("acked").is_empty());
    {
        let events = received.lock().expect("lock");
        assert_eq!(events.len(), 257);
        assert_eq!(
            events.last().expect("last").phase,
            ActivityPhase::NeedsAttention
        );
        assert_eq!(events[0], saved[0]);
    }
    reopened.close().await;
    server.abort();
}
#[tokio::test]
async fn failed_activity_acknowledgments_leave_the_original_batch_pending() {
    for response in [json!({}), json!([900])] {
        let f = running().await;
        f.journal
            .record_activity(
                id(6),
                &instance().to_string(),
                LocalProvider::Claude,
                ActivityPhase::ReadingFiles,
            )
            .await
            .expect("observe");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/v1/activity",
                    post(move || {
                        let response = response.clone();
                        async move { Json(response) }
                    }),
                ),
            )
            .await
            .expect("serve");
        });
        let remote = crate::sync::Remote::new(&origin, &"a".repeat(64).try_into().expect("token"))
            .expect("remote");
        assert!(remote.upload_activity(&f.journal).await.is_err());
        assert_eq!(
            f.journal.pending_activity().await.expect("pending").len(),
            1
        );
        server.abort();
    }
}
#[tokio::test]
async fn unavailable_activity_storage_never_invents_progress() {
    let f = running().await;
    let remote = crate::sync::Remote::new(
        "http://127.0.0.1:1",
        &"a".repeat(64).try_into().expect("token"),
    )
    .expect("remote");
    f.journal
        .record_activity(
            id(6),
            &instance().to_string(),
            LocalProvider::Codex,
            ActivityPhase::Working,
        )
        .await
        .expect("record");
    assert!(remote.upload_activity(&f.journal).await.is_err());
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&f.journal.pool)
        .await
        .expect("fault setup");
    sqlx::query("UPDATE execution_activity SET command_id='invalid'")
        .execute(&f.journal.pool)
        .await
        .expect("corrupt");
    assert!(f.journal.pending_activity().await.is_err());
    assert!(remote.upload_activity(&f.journal).await.is_err());
    f.journal.close().await;
    assert!(
        f.journal
            .record_activity(id(6), "stale", LocalProvider::Codex, ActivityPhase::Working)
            .await
            .is_err()
    );
    assert!(f.journal.pending_activity().await.is_err());
    assert!(remote.upload_activity(&f.journal).await.is_err());
    assert!(
        remote
            .synchronize(
                &f.journal,
                &orchestrator_protocol::sync::NodeProfile {
                    platform: orchestrator_protocol::capabilities::Platform::Linux,
                    capabilities: std::collections::BTreeSet::default(),
                    capacity: 1,
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn activity_transactions_roll_back_statement_and_commit_failures() {
    for fault in [
        "ALTER TABLE command_receipts RENAME COLUMN status TO broken",
        "CREATE TRIGGER reject_activity BEFORE INSERT ON execution_activity BEGIN SELECT RAISE(ABORT,'write failed'); END",
        "CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER INSERT ON execution_activity BEGIN INSERT INTO fault_child VALUES(1); END",
    ] {
        let f = running().await;
        sqlx::raw_sql(fault)
            .execute(&f.journal.pool)
            .await
            .expect("fault");
        assert!(
            f.journal
                .record_activity(
                    id(6),
                    &instance().to_string(),
                    LocalProvider::Codex,
                    ActivityPhase::Working
                )
                .await
                .is_err()
        );
        assert!(
            f.journal
                .pending_activity()
                .await
                .expect("rolled back")
                .is_empty()
        );
    }
}
#[tokio::test]
async fn a_remote_commit_is_retried_when_local_acknowledgment_cannot_commit() {
    for mode in 0..3 {
        let f = running().await;
        f.journal
            .record_activity(
                id(6),
                &instance().to_string(),
                LocalProvider::Codex,
                ActivityPhase::Working,
            )
            .await
            .expect("record");
        if mode == 1 {
            sqlx::raw_sql("CREATE TRIGGER reject_ack BEFORE UPDATE ON execution_activity BEGIN SELECT RAISE(ABORT,'ack failed'); END").execute(&f.journal.pool).await.expect("fault");
        } else if mode == 2 {
            sqlx::raw_sql("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER UPDATE ON execution_activity BEGIN INSERT INTO fault_child VALUES(1); END").execute(&f.journal.pool).await.expect("fault");
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let journal = f.journal.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/v1/activity",
                    post(move |Json(events): Json<Vec<ExecutionActivity>>| {
                        let journal = journal.clone();
                        async move {
                            if mode == 0 {
                                journal.close().await;
                            }
                            Json(events.iter().map(|e| e.sequence).collect::<Vec<_>>())
                        }
                    }),
                ),
            )
            .await
            .expect("serve");
        });
        let remote = crate::sync::Remote::new(&origin, &"a".repeat(64).try_into().expect("token"))
            .expect("remote");
        assert!(remote.upload_activity(&f.journal).await.is_err());
        let reopened = Journal::attach(f.journal.path()).await.expect("reopen");
        assert_eq!(
            reopened.pending_activity().await.expect("retained").len(),
            1
        );
        reopened.close().await;
        server.abort();
    }
}

#[tokio::test]
async fn broken_artifact_storage_prevents_publishing_a_completion_after_activity_sync() {
    let f = running().await;
    sqlx::query("DROP TABLE local_artifacts")
        .execute(&f.journal.pool)
        .await
        .expect("fault");
    let remote = crate::sync::Remote::new(
        "http://127.0.0.1:1",
        &"a".repeat(64).try_into().expect("token"),
    )
    .expect("remote");
    assert!(
        remote
            .synchronize(
                &f.journal,
                &orchestrator_protocol::sync::NodeProfile {
                    platform: orchestrator_protocol::capabilities::Platform::Linux,
                    capabilities: std::collections::BTreeSet::new(),
                    capacity: 1,
                }
            )
            .await
            .is_err()
    );
}
