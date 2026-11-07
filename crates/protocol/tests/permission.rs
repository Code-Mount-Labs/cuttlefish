//! Permission scope is cryptographically bound and rejects unbounded text.
#![allow(clippy::expect_used)]
use orchestrator_protocol::{
    permission::{PermissionRequest, ProviderBinding},
    values::MAX_MESSAGE_BYTES,
};
use serde_json::json;
mod fixtures;

#[test]
fn digest_binds_every_scope_field_and_preserves_exact_action_bytes() {
    let command = fixtures::report().events.remove(0).command;
    let request = PermissionRequest {
        permission_id: fixtures::id(200),
        command_id: command.command_id,
        binding: ProviderBinding {
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            provider: "test.provider".to_owned().try_into().expect("provider"),
            provider_session_id: "external-1".to_owned().try_into().expect("session"),
            resumable: true,
        },
        provider_request_id: "callback-1".to_owned().try_into().expect("callback"),
        action: "write report.txt".to_owned().try_into().expect("action"),
        expires_at_ms: 4_000_000_000_000,
    };
    let value = serde_json::to_value(&request).expect("serialize");
    let decoded: PermissionRequest = serde_json::from_value(value.clone()).expect("round trip");
    assert_eq!(request, decoded);
    assert_eq!(request.digest(), decoded.digest());
    assert_eq!(request.digest().len(), 64);
    let other = json!("00000000-0000-0000-0000-000000000999");
    for (path, replacement) in [
        ("/permission_id", other.clone()),
        ("/command_id", other.clone()),
        ("/binding/group_id", other.clone()),
        ("/binding/node_id", other.clone()),
        ("/binding/target/task_id", other.clone()),
        ("/binding/target/session_id", other),
        ("/binding/target/execution_epoch", json!(2)),
        ("/binding/provider", json!("other.provider")),
        ("/binding/provider_session_id", json!("external-2")),
        ("/binding/resumable", json!(false)),
        ("/provider_request_id", json!("callback-2")),
        ("/action", json!("write report.txt ")),
        ("/expires_at_ms", json!(4_000_000_000_001_i64)),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(path).expect("field") = replacement;
        let changed: PermissionRequest = serde_json::from_value(changed).expect("changed request");
        assert_ne!(request.digest(), changed.digest(), "unbound field: {path}");
    }
    for path in [
        "/action",
        "/provider_request_id",
        "/binding/provider_session_id",
    ] {
        for invalid in [String::new(), "x".repeat(MAX_MESSAGE_BYTES + 1)] {
            let mut changed = value.clone();
            *changed.pointer_mut(path).expect("field") = json!(invalid);
            assert!(serde_json::from_value::<PermissionRequest>(changed).is_err());
        }
    }
    let mut unknown = value;
    unknown["approved"] = json!(true);
    assert!(serde_json::from_value::<PermissionRequest>(unknown).is_err());
}
