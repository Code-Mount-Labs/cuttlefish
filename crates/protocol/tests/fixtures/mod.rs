#![allow(dead_code, clippy::expect_used)]
use orchestrator_protocol::sync::{NodeToken, SyncRequest};

pub fn token() -> NodeToken {
    "a".repeat(64).try_into().expect("test token")
}

pub fn id<T: std::str::FromStr>(value: u128) -> T
where
    T::Err: std::fmt::Debug,
{
    format!("{value:032x}").parse().expect("test identity")
}
pub fn goal() -> orchestrator_protocol::collaboration::GoalSpec {
    serde_json::from_value(serde_json::json!({"goal_id":id::<orchestrator_protocol::ids::GoalId>(100),"thread_id":id::<orchestrator_protocol::ids::ThreadId>(101),"objective":"Create a researched report and an application","acceptance":"Citations checked and all checks pass"})).expect("goal")
}
pub fn task(seed: u128) -> orchestrator_protocol::collaboration::TaskSpec {
    serde_json::from_value(serde_json::json!({"request":{"command_id":id::<orchestrator_protocol::ids::CommandId>(seed),"thread_id":id::<orchestrator_protocol::ids::ThreadId>(seed+1),"action":{"type":"start","task_id":id::<orchestrator_protocol::ids::TaskId>(seed+2),"session_id":id::<orchestrator_protocol::ids::SessionId>(seed+3),"workspace_id":id::<orchestrator_protocol::ids::WorkspaceId>(5),"capabilities":["code.rust"],"message":{"id":id::<orchestrator_protocol::ids::MessageId>(seed+4),"text":"Make the requested artifact"}}},"dependencies":[],"acceptance":"Independent review passed"})).expect("task")
}
pub fn release(seed: u128) -> orchestrator_protocol::collaboration::CapabilityRelease {
    serde_json::from_value(serde_json::json!({"release_id":id::<orchestrator_protocol::ids::ReleaseId>(seed),"name":"Test harness v1","capability":"code.rust","supersedes":null,"recipe":"Organization reviewed local adapter, test fixture only","instructions":"Produce the artifact and run the acceptance tests","evaluation":"All conformance and integration tests must pass"})).expect("release")
}
pub fn change(
    view: &orchestrator_protocol::collaboration::GoalView,
    action: orchestrator_protocol::collaboration::GoalAction,
) -> orchestrator_protocol::collaboration::GoalCommand {
    orchestrator_protocol::collaboration::GoalCommand {
        command_id: id(10000 + u128::from(view.revision)),
        expected_revision: view.revision,
        action,
    }
}

pub fn report() -> SyncRequest {
    serde_json::from_value(serde_json::json!({
        "version":1,"group_id":"00000000-0000-0000-0000-000000000001","node_id":"00000000-0000-0000-0000-000000000002",
        "profile":{"platform":"linux","capabilities":["code.rust"],"capacity":4},"active_sessions":1,
        "events":[{"sequence":1,"command":{
            "version":1,"command_id":"00000000-0000-0000-0000-000000000006","group_id":"00000000-0000-0000-0000-000000000001","node_id":"00000000-0000-0000-0000-000000000002",
            "target":{"task_id":"00000000-0000-0000-0000-000000000003","session_id":"00000000-0000-0000-0000-000000000004","execution_epoch":1},
            "action":{"type":"start","workspace_id":"00000000-0000-0000-0000-000000000005","message":{"id":"00000000-0000-0000-0000-000000000007","text":"hello"}}
        },"outcome":{"type":"completed","event":{
            "version":1,"command_id":"00000000-0000-0000-0000-000000000006","group_id":"00000000-0000-0000-0000-000000000001","node_id":"00000000-0000-0000-0000-000000000002",
            "target":{"task_id":"00000000-0000-0000-0000-000000000003","session_id":"00000000-0000-0000-0000-000000000004","execution_epoch":1},
            "kind":{"type":"replied","message_id":"00000000-0000-0000-0000-000000000007","text":"hello"}
        }}}]
    })).expect("valid fixture")
}
