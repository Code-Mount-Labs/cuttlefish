//! Synchronization boundaries, correlations, and credential redaction.
#![allow(clippy::expect_used)]
mod fixtures;
use orchestrator_protocol::{
    agent::{AgentAction, AgentEventKind},
    sync::{CommandOutcome, NodeToken, SyncRequest},
};

#[test]
fn tokens_are_validated_and_never_debug_logged() {
    let token = fixtures::token();
    assert_eq!(token.expose(), "a".repeat(64));
    assert_eq!(format!("{token:?}"), "NodeToken([REDACTED])");
    for value in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "G".repeat(64),
        "\n".repeat(64),
    ] {
        let error = NodeToken::try_from(value).expect_err("invalid token");
        assert!(error.to_string().contains("64"));
    }
    assert!(NodeToken::try_from("0123456789abcdef".repeat(4)).is_ok());
}

#[test]
fn reports_enforce_bounds_and_unique_identities() {
    let original = fixtures::report();
    original.validate().expect("valid");
    let decoded: SyncRequest =
        serde_json::from_slice(&serde_json::to_vec(&original).expect("serialize")).expect("decode");
    assert_eq!(original, decoded);
    let mut report = original.clone();
    report.events.clear();
    report.validate().expect("heartbeat only");
    for case in 0..8 {
        let mut report = original.clone();
        match case {
            0 => report.events = vec![report.events[0].clone(); 5],
            1 => report.profile.capacity = 0,
            2 => {
                report.profile.capabilities = (0..65)
                    .map(|n| format!("cap{n}").try_into().expect("cap"))
                    .collect();
            }
            3 => report.events[0].sequence = 0,
            4 => report.events.push(report.events[0].clone()),
            5 => {
                let mut duplicate = report.events[0].clone();
                duplicate.sequence = 2;
                report.events.push(duplicate);
            }
            6 => {
                report.events[0].command.group_id =
                    "00000000-0000-0000-0000-000000000099".parse().expect("id");
            }
            _ => {
                report.events[0].command.node_id =
                    "00000000-0000-0000-0000-000000000099".parse().expect("id");
            }
        }
        assert!(
            !report
                .validate()
                .expect_err("invalid report")
                .to_string()
                .is_empty()
        );
    }
    let mut value = serde_json::to_value(original).expect("value");
    value["version"] = 2.into();
    assert!(serde_json::from_value::<SyncRequest>(value).is_err());
}

#[test]
fn results_must_correlate_and_rejections_are_bounded() {
    for reason in ["rejected".into(), String::new(), "x".repeat(65_537)] {
        let valid = reason == "rejected";
        let mut report = fixtures::report();
        report.events[0].outcome = CommandOutcome::Rejected { reason };
        assert_eq!(report.validate().is_ok(), valid);
    }
    for case in 0..8 {
        let mut report = fixtures::report();
        let item = &mut report.events[0];
        let CommandOutcome::Completed { event } = &mut item.outcome else {
            unreachable!()
        };
        match case {
            0 => event.node_id = "00000000-0000-0000-0000-000000000099".parse().expect("id"),
            1 => event.group_id = "00000000-0000-0000-0000-000000000099".parse().expect("id"),
            2 => event.command_id = "00000000-0000-0000-0000-000000000099".parse().expect("id"),
            3 => event.target.execution_epoch = event.target.execution_epoch.next().expect("epoch"),
            4 => event.kind = AgentEventKind::Cancelled,
            5 => {
                item.command.action = AgentAction::Cancel;
                event.kind = AgentEventKind::Cancelled;
            }
            6 => {
                let AgentAction::Start { message, .. } = &item.command.action else {
                    unreachable!()
                };
                item.command.action = AgentAction::Send {
                    message: message.clone(),
                };
            }
            _ => {
                let AgentEventKind::Replied { message_id, .. } = &mut event.kind else {
                    unreachable!()
                };
                *message_id = "00000000-0000-0000-0000-000000000099".parse().expect("id");
            }
        }
        assert_eq!(report.validate().is_ok(), matches!(case, 5 | 6));
    }
}
