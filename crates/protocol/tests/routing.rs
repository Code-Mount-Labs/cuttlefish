//! Routing wire contracts preserve exact operation and execution identities.
#![allow(clippy::expect_used)]
use orchestrator_protocol::routing::{DispatchBatch, DispatchPoll, RouteRequest};
use serde_json::json;

#[test]
fn routing_actions_round_trip_and_reject_unknown_fields() {
    let message = json!({"id":"00000000-0000-0000-0000-000000000004","text":"continue"});
    for action in [
        json!({"type":"start","task_id":"00000000-0000-0000-0000-000000000003","session_id":"00000000-0000-0000-0000-000000000005","workspace_id":"00000000-0000-0000-0000-000000000006","capabilities":["code.rust"],"message":message}),
        json!({"type":"send","message":message}),
        json!({"type":"cancel"}),
        json!({"type":"restart","session_id":"00000000-0000-0000-0000-000000000007","message":message}),
    ] {
        let value = json!({"command_id":"00000000-0000-0000-0000-000000000001","thread_id":"00000000-0000-0000-0000-000000000002","action":action});
        let request: RouteRequest = serde_json::from_value(value.clone()).expect("contract");
        assert_eq!(serde_json::to_value(&request).expect("serialize"), value);
        let mut invalid = value.clone();
        invalid["extra"] = json!(true);
        assert!(serde_json::from_value::<RouteRequest>(invalid).is_err());
        let mut invalid = value;
        invalid["action"]["extra"] = json!(true);
        assert!(serde_json::from_value::<RouteRequest>(invalid).is_err());
    }
    assert!(
        serde_json::from_value::<DispatchPoll>(
            json!({"workspaces":[],"active_sessions":-1,"accepted":[]})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<DispatchBatch>(
            json!({"acknowledged":[],"command":null,"extra":true})
        )
        .is_err()
    );
}
