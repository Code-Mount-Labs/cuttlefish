#![allow(clippy::expect_used)]
use super::*;
use crate::test_support::{Fixture, id, instance};
use axum::{Json, Router, routing::post};
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
async fn tools_survive_restart_and_authenticate_exact_retries() {
    let f = running().await;
    let owner = instance().to_string();
    for call in [String::new(), "x".repeat(257)] {
        assert!(
            f.journal
                .stage_tool(id(6), &owner, &call, ToolAction::Agents {})
                .await
                .is_err()
        );
    }
    assert!(
        f.journal
            .stage_tool(id(6), "stale", "a", ToolAction::Agents {})
            .await
            .is_err()
    );
    let first = f
        .journal
        .stage_tool(id(6), &owner, "a", ToolAction::Agents {})
        .await
        .expect("stage");
    let reopened = Journal::attach(f.journal.path()).await.expect("reopen");
    assert_eq!(
        reopened
            .stage_tool(id(6), &owner, "a", ToolAction::Agents {})
            .await
            .expect("retry"),
        first
    );
    assert!(
        reopened
            .stage_tool(id(6), &owner, "a", ToolAction::Task { task_id: id(99) })
            .await
            .is_err()
    );
    assert!(
        reopened
            .tool_response(id(6), "a")
            .await
            .expect("pending")
            .is_none()
    );
    assert!(wait_reply(&reopened, id(6), "a", 0).await.is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let expected = first.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/agent-tools",
                post(
                    move |headers: axum::http::HeaderMap, Json(request): Json<ToolRequest>| {
                        let expected = expected.clone();
                        async move {
                            assert_eq!(request, expected);
                            assert!(headers.contains_key("authorization"));
                            Json(json!({"agents":[]}))
                        }
                    },
                ),
            ),
        )
        .await
        .expect("serve");
    });
    let remote = Remote::new(&origin, &"a".repeat(64).try_into().expect("token")).expect("remote");
    check_failed_response_write(&remote, &reopened).await;
    remote.flush_tools(&reopened).await.expect("flush");
    let result = invoke(
        &reopened,
        id(6),
        &owner,
        "a",
        "cuttlefish_agents",
        json!({}),
    )
    .await
    .expect("cached result");
    assert_eq!(result, json!({"value":{"agents":[]}}));
    remote.flush_tools(&reopened).await.expect("empty outbox");
    server.abort();
    reopened.close().await;
    assert!(reopened.tool_response(id(6), "a").await.is_err());
    assert!(remote.flush_tools(&reopened).await.is_err());
    assert!(
        reopened
            .stage_tool(id(6), &owner, "a", ToolAction::Agents {})
            .await
            .is_err()
    );
}
#[tokio::test]
async fn transport_failure_retains_request_and_definitive_rejection_is_saved() {
    let f = running().await;
    f.journal
        .stage_tool(id(6), &instance().to_string(), "a", ToolAction::Agents {})
        .await
        .expect("stage");
    let token = "a".repeat(64).try_into().expect("token");
    let offline = Remote::new("http://127.0.0.1:1", &token).expect("remote");
    assert!(offline.flush_tools(&f.journal).await.is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/v1/agent-tools",
                post(|| async { axum::http::StatusCode::FORBIDDEN }),
            ),
        )
        .await
        .expect("serve");
    });
    Remote::new(&origin, &token)
        .expect("remote")
        .flush_tools(&f.journal)
        .await
        .expect("definitive rejection");
    assert!(
        f.journal
            .tool_response(id(6), "a")
            .await
            .expect("response")
            .expect("saved")
            .get("error")
            .is_some()
    );
    server.abort();
}

#[tokio::test]
async fn malformed_tool_calls_fail_before_dispatch() {
    let f = running().await;
    let reopened = f.journal;
    let owner = instance().to_string();
    for (name, args) in [
        ("bad", json!({})),
        ("cuttlefish_agents", json!([])),
        ("cuttlefish_agents", json!({"type":"delegate"})),
        ("cuttlefish_task", json!({})),
        ("cuttlefish_delegate", json!({})),
    ] {
        assert!(
            invoke(&reopened, id(6), &owner, "bad", name, args)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn a_slow_router_returns_stable_pending_identity_without_resubmitting() {
    let f = running().await;
    let owner = instance().to_string();
    let pending=invoke_with_limit(&f.journal,id(6),&owner,"pending","cuttlefish_delegate",json!({"workspace_id":id::<orchestrator_protocol::ids::WorkspaceId>(5),"message":"Build it","acceptance":"Verified result"}),1).await.expect("pending");
    assert_eq!(pending["pending"], true);
    assert_eq!(pending["task_id"], pending["request_id"]);
    let status = invoke_with_limit(
        &f.journal,
        id(6),
        &owner,
        "read",
        "cuttlefish_agents",
        json!({}),
        0,
    )
    .await
    .expect("pending read");
    assert!(status["task_id"].is_null());
    // If the response table becomes unavailable after staging, do not turn storage failure into pending success.
    sqlx::raw_sql("CREATE TRIGGER lose_tools AFTER INSERT ON provider_tools BEGIN DELETE FROM provider_tools WHERE sequence=NEW.sequence; END;").execute(&f.journal.pool).await.expect("fault");
    assert!(
        invoke_with_limit(
            &f.journal,
            id(6),
            &owner,
            "lost",
            "cuttlefish_agents",
            json!({}),
            1
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn tool_writes_fail_atomically_and_idle_sessions_release_capacity() {
    for fault in [
        "ALTER TABLE local_sessions RENAME COLUMN instance_id TO hidden",
        "CREATE TRIGGER reject_tool BEFORE INSERT ON provider_tools BEGIN SELECT RAISE(ABORT,'insert failed'); END",
        "PRAGMA defer_foreign_keys=ON; CREATE TABLE tool_parent(id INTEGER PRIMARY KEY); CREATE TABLE tool_child(id INTEGER REFERENCES tool_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER reject_commit AFTER INSERT ON provider_tools BEGIN INSERT INTO tool_child VALUES(1); END",
    ] {
        let f = running().await;
        sqlx::raw_sql(fault)
            .execute(&f.journal.pool)
            .await
            .expect("fault");
        assert!(
            f.journal
                .stage_tool(
                    id(6),
                    &instance().to_string(),
                    "fault",
                    ToolAction::Agents {}
                )
                .await
                .is_err()
        );
        assert!(
            invoke(
                &f.journal,
                id(6),
                &instance().to_string(),
                "fault",
                "cuttlefish_agents",
                json!({})
            )
            .await
            .is_err()
        );
    }
    let f = running().await;
    assert_eq!(
        f.journal
            .dispatch_poll()
            .await
            .expect("busy")
            .active_sessions,
        1
    );
    f.journal
        .complete(
            id(6),
            &instance().to_string(),
            &crate::test_support::outcome(&crate::test_support::start()),
        )
        .await
        .expect("finish");
    assert_eq!(
        f.journal
            .dispatch_poll()
            .await
            .expect("idle")
            .active_sessions,
        0
    );
    assert_eq!(
        f.session().await.state,
        crate::model::SessionStatus::Running
    );
}

async fn check_failed_response_write(remote: &Remote, reopened: &Journal) {
    sqlx::query("CREATE TRIGGER reject_response BEFORE UPDATE ON provider_tools BEGIN SELECT RAISE(ABORT,'response failed'); END").execute(&reopened.pool).await.expect("fault");
    assert!(remote.flush_tools(reopened).await.is_err());
    assert!(
        reopened
            .tool_response(id(6), "a")
            .await
            .expect("retained")
            .is_none()
    );
    sqlx::query("DROP TRIGGER reject_response")
        .execute(&reopened.pool)
        .await
        .expect("repair");
}
