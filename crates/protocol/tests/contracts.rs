//! Boundary tests for every public wire type and validation rule.
#![allow(clippy::expect_used)]

use std::{error::Error, fmt::Debug, str::FromStr};

use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind, Message, SessionTarget},
    capabilities::{Capability, InvalidCapability, Platform},
    ids::{
        CommandId, GroupId, IdError, MessageId, NodeId, SessionId, TaskId, ThreadId, WorkspaceId,
    },
    task::TaskStatus,
    values::{Epoch, MAX_MESSAGE_BYTES, MessageText, ProtocolVersion, ValueError},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use uuid::Uuid;

fn id<T: FromStr>(value: u128) -> T
where
    T::Err: Debug,
{
    format!("{value:032x}")
        .parse()
        .expect("valid fixture identity")
}

fn round_trip<T: Serialize + DeserializeOwned + Debug + PartialEq>(value: &T) {
    let encoded = serde_json::to_string(value).expect("serialize fixture");
    let decoded: T = serde_json::from_str(&encoded).expect("deserialize fixture");
    assert_eq!(&decoded, value);
}

macro_rules! test_id {
    ($test:ident, $ty:ty) => {
        #[test]
        fn $test() {
            let raw = Uuid::from_u128(1);
            let typed = <$ty>::try_from(raw).expect("non-nil UUID");
            assert_eq!(Uuid::from(typed), raw);
            assert_eq!(typed.to_string(), "00000000-0000-0000-0000-000000000001");
            assert_eq!(
                typed.to_string().parse::<$ty>().expect("parse display"),
                typed
            );
            assert_eq!(
                serde_json::to_value(typed).expect("serialize ID"),
                json!(typed.to_string())
            );
            assert!(matches!(<$ty>::try_from(Uuid::nil()), Err(IdError::Nil)));
            assert!(matches!("bad-id".parse::<$ty>(), Err(IdError::Invalid(_))));
            assert!(matches!(
                Uuid::nil().to_string().parse::<$ty>(),
                Err(IdError::Nil)
            ));
            assert!(serde_json::from_value::<$ty>(json!(Uuid::nil().to_string())).is_err());
            assert!(serde_json::from_value::<$ty>(json!("not-a-uuid")).is_err());
            assert!(serde_json::from_value::<$ty>(json!(1)).is_err());
            round_trip(&typed);
        }
    };
}

test_id!(group_identity, GroupId);
test_id!(node_identity, NodeId);
test_id!(thread_identity, ThreadId);
test_id!(task_identity, TaskId);
test_id!(session_identity, SessionId);
test_id!(command_identity, CommandId);
test_id!(message_identity, MessageId);
test_id!(workspace_identity, WorkspaceId);
test_id!(
    permission_identity,
    orchestrator_protocol::ids::PermissionId
);
test_id!(reviewer_identity, orchestrator_protocol::ids::ReviewerId);

#[test]
fn identifier_errors_preserve_their_source() {
    let error = "bad".parse::<GroupId>().expect_err("invalid ID");
    assert!(error.to_string().starts_with("invalid UUID:"));
    assert!(error.source().is_some());
    assert_eq!(
        IdError::Nil.to_string(),
        "resource identifiers must not be nil"
    );
    assert!(IdError::Nil.source().is_none());
}

#[test]
fn message_text_preserves_content_and_enforces_byte_boundaries() {
    let text = MessageText::try_from("  नमस्ते\nworld  ".to_owned()).expect("valid Unicode");
    assert_eq!(text.as_str(), "  नमस्ते\nworld  ");
    round_trip(&text);
    assert_eq!(String::from(text), "  नमस्ते\nworld  ");

    for empty in ["", " ", "\n\t", "\u{2003}"] {
        assert_eq!(
            MessageText::try_from(empty.to_owned()),
            Err(ValueError::EmptyText)
        );
        assert!(serde_json::from_value::<MessageText>(json!(empty)).is_err());
    }
    for boundary in [
        "x".repeat(MAX_MESSAGE_BYTES),
        "é".repeat(MAX_MESSAGE_BYTES / 2),
    ] {
        assert!(MessageText::try_from(boundary).is_ok());
    }
    for too_long in [
        "x".repeat(MAX_MESSAGE_BYTES + 1),
        "é".repeat(MAX_MESSAGE_BYTES / 2 + 1),
    ] {
        assert_eq!(
            MessageText::try_from(too_long.clone()),
            Err(ValueError::TextTooLong)
        );
        assert!(serde_json::from_value::<MessageText>(json!(too_long)).is_err());
    }
}

#[test]
fn epochs_never_accept_zero_or_wrap() {
    let initial = Epoch::INITIAL;
    assert_eq!(initial.get(), 1);
    assert_eq!(u64::from(initial), 1);
    assert_eq!(initial.to_string(), "1");
    assert_eq!(initial.next().expect("increment").get(), 2);
    assert_eq!(Epoch::try_from(0), Err(ValueError::ZeroEpoch));
    let last = Epoch::try_from(u64::MAX).expect("maximum epoch is valid");
    assert_eq!(last.next(), Err(ValueError::EpochExhausted));
    round_trip(&initial);
    round_trip(&last);
    for invalid in [json!(0), json!(-1), json!(1.5), json!("1"), Value::Null] {
        assert!(serde_json::from_value::<Epoch>(invalid).is_err());
    }
}

#[test]
fn protocol_versions_fail_closed() {
    assert_eq!(u16::from(ProtocolVersion::V1), 1);
    assert_eq!(ProtocolVersion::try_from(1), Ok(ProtocolVersion::V1));
    round_trip(&ProtocolVersion::V1);
    for unsupported in [0, 2, u16::MAX] {
        assert_eq!(
            ProtocolVersion::try_from(unsupported),
            Err(ValueError::UnsupportedVersion(unsupported))
        );
        assert!(serde_json::from_value::<ProtocolVersion>(json!(unsupported)).is_err());
    }
}

#[test]
fn value_errors_have_actionable_descriptions() {
    for (error, expected) in [
        (
            ValueError::EmptyText,
            "message text must contain a non-whitespace character".to_owned(),
        ),
        (
            ValueError::TextTooLong,
            format!("message text exceeds {MAX_MESSAGE_BYTES} UTF-8 bytes"),
        ),
        (
            ValueError::ZeroEpoch,
            "ownership epochs start at 1".to_owned(),
        ),
        (
            ValueError::EpochExhausted,
            "ownership epoch exhausted".to_owned(),
        ),
        (
            ValueError::UnsupportedVersion(2),
            "unsupported protocol version: 2".to_owned(),
        ),
    ] {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
    }
}

#[test]
fn capability_names_are_canonical_and_bounded() {
    for valid in [
        "a".to_owned(),
        "build.ios_18-arm64".to_owned(),
        "a".repeat(64),
    ] {
        let capability = Capability::try_from(valid.clone()).expect("valid capability");
        assert_eq!(capability.as_str(), valid);
        round_trip(&capability);
        assert_eq!(String::from(capability), valid);
    }
    for invalid in [
        String::new(),
        "9ios".to_owned(),
        "Xcode".to_owned(),
        "build ios".to_owned(),
        ".ios".to_owned(),
        "é".to_owned(),
        "a/ios".to_owned(),
        "a".repeat(65),
    ] {
        assert_eq!(
            Capability::try_from(invalid.clone()),
            Err(InvalidCapability)
        );
        assert!(serde_json::from_value::<Capability>(json!(invalid)).is_err());
    }
    assert_eq!(
        InvalidCapability.to_string(),
        "capability must match [a-z][a-z0-9._-]{0,63}"
    );
    assert!(InvalidCapability.source().is_none());
}

#[test]
fn platform_and_task_state_spellings_are_stable() {
    for (platform, name) in [
        (Platform::MacOs, "mac_os"),
        (Platform::Windows, "windows"),
        (Platform::Linux, "linux"),
    ] {
        assert_eq!(
            serde_json::to_value(platform).expect("platform"),
            json!(name)
        );
        round_trip(&platform);
    }
    for (status, name) in [
        (TaskStatus::Queued, "queued"),
        (TaskStatus::Running, "running"),
        (TaskStatus::WaitingInput, "waiting_input"),
        (TaskStatus::Cancelling, "cancelling"),
        (TaskStatus::Completed, "completed"),
        (TaskStatus::Failed, "failed"),
        (TaskStatus::Interrupted, "interrupted"),
        (TaskStatus::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(status).expect("status"), json!(name));
        round_trip(&status);
    }
    assert!(serde_json::from_value::<Platform>(json!("unknown")).is_err());
    assert!(serde_json::from_value::<TaskStatus>(json!("done")).is_err());
}

fn start_command() -> AgentCommand {
    AgentCommand {
        version: ProtocolVersion::V1,
        command_id: id(1),
        group_id: id(2),
        node_id: id(3),
        target: SessionTarget {
            task_id: id(4),
            session_id: id(5),
            execution_epoch: Epoch::INITIAL,
        },
        action: AgentAction::Start {
            workspace_id: id(6),
            message: Message {
                id: id(7),
                text: "hello".to_owned().try_into().expect("valid text"),
            },
        },
    }
}

#[test]
fn start_command_matches_the_published_wire_shape() {
    let command = start_command();
    assert_eq!(
        serde_json::to_value(&command).expect("command"),
        json!({
            "version": 1,
            "command_id": "00000000-0000-0000-0000-000000000001",
            "group_id": "00000000-0000-0000-0000-000000000002",
            "node_id": "00000000-0000-0000-0000-000000000003",
            "target": {
                "task_id": "00000000-0000-0000-0000-000000000004",
                "session_id": "00000000-0000-0000-0000-000000000005",
                "execution_epoch": 1
            },
            "action": {
                "type": "start",
                "workspace_id": "00000000-0000-0000-0000-000000000006",
                "message": { "id": "00000000-0000-0000-0000-000000000007", "text": "hello" }
            }
        })
    );
    round_trip(&command);
}

#[test]
fn all_commands_and_events_round_trip() {
    let mut command = start_command();
    let message = Message {
        id: id(8),
        text: "follow up".to_owned().try_into().expect("text"),
    };
    for action in [
        command.action.clone(),
        AgentAction::Send {
            message: message.clone(),
        },
        AgentAction::Cancel,
    ] {
        command.action = action;
        round_trip(&command);
    }
    for kind in [
        AgentEventKind::Replied {
            message_id: message.id,
            text: message.text,
        },
        AgentEventKind::Cancelled,
    ] {
        let event = AgentEvent {
            version: command.version,
            command_id: command.command_id,
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            kind,
        };
        round_trip(&event);
        let mut value = serde_json::to_value(event).expect("event");
        value["unexpected"] = json!(true);
        assert!(serde_json::from_value::<AgentEvent>(value).is_err());
    }
}

#[test]
fn malformed_commands_cannot_bypass_validated_types() {
    let fixture = serde_json::to_value(start_command()).expect("fixture");
    for pointer in ["", "/target", "/action", "/action/message"] {
        let mut value = fixture.clone();
        value.pointer_mut(pointer).expect("fixture location")["unexpected"] = json!(true);
        assert!(
            serde_json::from_value::<AgentCommand>(value).is_err(),
            "unknown field at {pointer}"
        );
    }
    for (pointer, invalid) in [
        ("/version", json!(2)),
        ("/target/execution_epoch", json!(0)),
        ("/group_id", json!(Uuid::nil().to_string())),
        ("/action/message/text", json!("  ")),
        ("/action/type", json!("shell")),
    ] {
        let mut value = fixture.clone();
        *value.pointer_mut(pointer).expect("fixture location") = invalid;
        assert!(
            serde_json::from_value::<AgentCommand>(value).is_err(),
            "invalid value at {pointer}"
        );
    }
    let mut missing = fixture;
    missing.as_object_mut().expect("object").remove("target");
    assert!(serde_json::from_value::<AgentCommand>(missing).is_err());
}
