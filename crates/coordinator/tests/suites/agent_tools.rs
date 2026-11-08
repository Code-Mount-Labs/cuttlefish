use crate::tests::{
    common::Fixture,
    routing::{Selection, finish, id, poll, ready, request},
};
use orchestrator_coordinator::{routing::RoutingService, store::Store};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    routing::{DispatchPoll, RouteAction},
    tools::{ToolAction, ToolRequest},
};
use std::sync::Arc;
#[tokio::test]
async fn real_peer_delegation_is_scoped_idempotent_and_recovered() {
    let (f, a, b, service, parent) = prepared().await;
    let mut tool = ToolRequest {
        parent: parent.command.command_id,
        id: id(300),
        action: ToolAction::Agents {},
    };
    assert!(
        service.agent_tool(&a, &tool).await.is_err(),
        "not executing yet"
    );
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    check_tool_http(&f, &a, &tool).await;
    assert_eq!(
        service.agent_tool(&a, &tool).await.expect("discover")["workspaces"]
            .as_array()
            .expect("array")
            .len(),
        2
    );
    assert!(
        service.agent_tool(&b, &tool).await.is_err(),
        "other node cannot impersonate parent"
    );
    tool.action = ToolAction::Delegate {
        workspace_id: id(15),
        message: "Build and verify the Linux part"
            .to_owned()
            .try_into()
            .expect("text"),
        acceptance: "Return verified Linux artifact"
            .to_owned()
            .try_into()
            .expect("text"),
    };
    let first = service.agent_tool(&a, &tool).await.expect("delegate");
    assert_eq!(
        first["node_id"],
        serde_json::json!(id::<orchestrator_protocol::ids::NodeId>(9))
    );
    let reopened = Store::open(&f.url).await.expect("reopen");
    let recovered = RoutingService::new(reopened.clone(), Arc::new(Selection::default()));
    assert_eq!(
        recovered.agent_tool(&a, &tool).await.expect("exact retry"),
        first
    );
    let children = reopened
        .conversation_tasks(&a, parent.request.thread_id)
        .await
        .expect("children");
    assert_eq!(children.len(), 1);
    let child = reopened.thread(&a, id(300)).await.expect("child");
    f.store
        .poll_dispatch(
            &b,
            &DispatchPoll {
                workspaces: vec![id(15)],
                ..poll()
            },
        )
        .await
        .expect("claim child");
    let mut recursive = tool.clone();
    recursive.parent = id(300);
    recursive.id = id(301);
    assert!(
        recovered.agent_tool(&b, &recursive).await.is_err(),
        "no recursive delegation"
    );
    finish(&f, &b, &child, 1).await;
    let result = ToolRequest {
        action: ToolAction::Task { task_id: id(300) },
        ..tool.clone()
    };
    assert_eq!(
        recovered.agent_tool(&a, &result).await.expect("result")["state"],
        "completed"
    );
    reject_changed_or_unauthorized(&f, &recovered, &a, tool).await;
    finish(&f, &a, &parent, 1).await;
    assert!(
        recovered.agent_tool(&a, &result).await.is_err(),
        "completed parent cannot invoke tools"
    );
    reopened.close().await;
    f.close().await;
}
#[tokio::test]
async fn followups_are_ordered_bounded_and_survive_router_restart() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let start = service.submit(&token, request(1000)).await.expect("start");
    f.store.poll_dispatch(&token, &poll()).await.expect("claim");
    let RouteAction::Start { message, .. } = start.request.action.clone() else {
        unreachable!()
    };
    for i in 0..31 {
        let mut req = start.request.clone();
        req.command_id = id(2000 + i);
        req.action = RouteAction::Send {
            message: message.clone(),
        };
        service.submit(&token, req).await.expect("enqueue");
    }
    let mut overflow = start.request.clone();
    overflow.command_id = id(3000);
    overflow.action = RouteAction::Send {
        message: message.clone(),
    };
    assert!(service.submit(&token, overflow).await.is_err());
    let mut acknowledged = poll();
    acknowledged.accepted = vec![start.command.command_id];
    assert!(
        f.store
            .poll_dispatch(&token, &acknowledged)
            .await
            .expect("wait for parent")
            .command
            .is_none()
    );
    let reopened = Store::open(&f.url).await.expect("reopen");
    finish(&f, &token, &start, 1).await;
    let next = reopened
        .poll_dispatch(&token, &poll())
        .await
        .expect("next")
        .command
        .expect("command");
    assert_eq!(next.command_id, id(2000));
    let mut ack = poll();
    ack.accepted = vec![next.command_id];
    assert!(
        reopened
            .poll_dispatch(&token, &ack)
            .await
            .expect("no concurrent turn")
            .command
            .is_none()
    );
    assert_eq!(
        reopened
            .conversation(&token, start.request.thread_id)
            .await
            .expect("saved messages")
            .len(),
        32
    );
    reopened.close().await;
    f.close().await;
}

pub(crate) async fn prepared() -> (
    Fixture,
    orchestrator_protocol::sync::NodeToken,
    orchestrator_protocol::sync::NodeToken,
    RoutingService,
    orchestrator_protocol::routing::DispatchView,
) {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 8).await;
    let b = ready(&f, 9, 8).await;
    for (token, workspace) in [(&a, 5), (&b, 15)] {
        f.store
            .advertise_workspaces(
                token,
                &[ProviderWorkspace {
                    workspace_id: id(workspace),
                    name: format!("Workspace {workspace}"),
                    provider: LocalProvider::Codex,
                    allow_edits: true,
                }],
            )
            .await
            .expect("workspace");
        f.store
            .poll_dispatch(
                token,
                &DispatchPoll {
                    workspaces: vec![id(workspace)],
                    ..poll()
                },
            )
            .await
            .expect("readiness");
    }
    f.sql(r#"UPDATE nodes SET profile=jsonb_set(profile,'{capabilities}',(profile->'capabilities') || '["codex"]'::jsonb)"#)
        .await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let parent = service.submit(&a, request(200)).await.expect("parent");
    (f, a, b, service, parent)
}

async fn reject_changed_or_unauthorized(
    f: &Fixture,
    recovered: &RoutingService,
    a: &orchestrator_protocol::sync::NodeToken,
    mut tool: ToolRequest,
) {
    let bad = ToolRequest {
        action: ToolAction::Task { task_id: id(999) },
        ..tool.clone()
    };
    assert!(recovered.agent_tool(a, &bad).await.is_err());
    if let ToolAction::Delegate { message, .. } = &mut tool.action {
        *message = "Different work".to_owned().try_into().expect("text");
    }
    assert!(
        recovered.agent_tool(a, &tool).await.is_err(),
        "identity cannot change work"
    );
    f.sql("UPDATE provider_workspaces SET workspace=jsonb_set(workspace,'{allow_edits}','false') WHERE workspace_id='00000000-0000-0000-0000-000000000005'").await;
    tool.id = id(302);
    assert!(
        recovered.agent_tool(a, &tool).await.is_err(),
        "readonly parent cannot grant writes"
    );
    tool.action = ToolAction::Delegate {
        workspace_id: id(998),
        message: "Work".to_owned().try_into().expect("text"),
        acceptance: "Evidence".to_owned().try_into().expect("text"),
    };
    assert!(
        recovered.agent_tool(a, &tool).await.is_err(),
        "unknown workspace"
    );
}

async fn check_tool_http(
    f: &Fixture,
    a: &orchestrator_protocol::sync::NodeToken,
    tool: &ToolRequest,
) {
    let server =
        crate::tests::common::Server::start(orchestrator_coordinator::web::router_with_selector(
            f.store.clone(),
            Arc::new(Selection::default()),
        ))
        .await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/v1/agent-tools", server.origin))
            .json(&tool)
            .send()
            .await
            .expect("response")
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert!(
        client
            .post(format!("{}/v1/agent-tools", server.origin))
            .bearer_auth(a.expose())
            .json(&tool)
            .send()
            .await
            .expect("tools response")
            .status()
            .is_success()
    );
    let rejected = ToolRequest {
        parent: id(999),
        ..tool.clone()
    };
    assert_eq!(
        client
            .post(format!("{}/v1/agent-tools", server.origin))
            .bearer_auth(a.expose())
            .json(&rejected)
            .send()
            .await
            .expect("rejected request")
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    server.close().await;
}

#[tokio::test]
async fn followups_do_not_disappear_behind_interrupted_or_ending_sessions() {
    for interrupted in [false, true] {
        let (f, a, _, service, parent) = prepared().await;
        f.store.poll_dispatch(&a, &poll()).await.expect("claim");
        let mut next = parent.request.clone();
        next.command_id = id(401);
        if interrupted {
            f.store
                .record_activity(
                    &a,
                    &[orchestrator_protocol::activity::ExecutionActivity {
                        sequence: 1,
                        command_id: parent.command.command_id,
                        phase: orchestrator_protocol::activity::ActivityPhase::NeedsAttention,
                        provider: LocalProvider::Codex,
                        at_ms: 1234,
                        summary: Some("Provider connection closed".into()),
                    }],
                )
                .await
                .expect("interrupted");
        } else {
            finish(&f, &a, &parent, 1).await;
            next.action = RouteAction::Cancel {};
            service
                .submit(&a, next.clone())
                .await
                .expect("cancel queued");
        }
        next.command_id = id(402);
        let RouteAction::Start { message, .. } = parent.request.action else {
            unreachable!()
        };
        next.action = RouteAction::Send { message };
        assert!(service.submit(&a, next).await.is_err());
        f.close().await;
    }
}
