//! Public adapter contract tests; no private implementation hooks are required.
#![allow(clippy::expect_used)]

use std::{error::Error, fmt::Debug, str::FromStr};

use orchestrator_agents::{
    Agent,
    fake::{FakeAgent, FakeAgentError as AgentError},
};
use orchestrator_domain::{TaskAction, TaskState};
use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind, Message, SessionTarget},
    ids::{CommandId, SessionId},
    values::{Epoch, ProtocolVersion},
};

fn id<T: FromStr>(value: u128) -> T
where
    T::Err: Debug,
{
    format!("{value:032x}").parse().expect("fixture ID")
}

fn message(number: u128, text: &str) -> Message {
    Message {
        attachments: Vec::new(),
        id: id(number),
        text: text.to_owned().try_into().expect("fixture text"),
    }
}

fn command(number: u128, action: AgentAction) -> AgentCommand {
    AgentCommand {
        version: ProtocolVersion::V1,
        command_id: id(number),
        group_id: id(1),
        node_id: id(2),
        target: SessionTarget {
            task_id: id(3),
            session_id: id(4),
            execution_epoch: Epoch::try_from(2).expect("epoch"),
        },
        action,
    }
}

fn start() -> AgentCommand {
    command(
        10,
        AgentAction::Start {
            workspace_id: id(5),
            message: message(6, "first request"),
        },
    )
}

fn send(number: u128) -> AgentCommand {
    command(
        number,
        AgentAction::Send {
            message: message(number, "follow-up"),
        },
    )
}

fn fake() -> FakeAgent {
    FakeAgent::new(id(1), id(2))
}

async fn execute<A: Agent<Error = AgentError>>(
    agent: &mut A,
    command: &AgentCommand,
) -> Result<AgentEvent, AgentError> {
    agent.execute(command).await
}

#[tokio::test]
async fn follows_up_in_the_original_session_and_preserves_wire_context() {
    let mut agent = fake();
    assert!(agent.session(id(4)).is_none());
    let first = start();
    let event = execute(&mut agent, &first).await.expect("start");
    assert_eq!(
        event,
        AgentEvent {
            artifacts: Vec::new(),
            version: first.version,
            command_id: first.command_id,
            group_id: first.group_id,
            node_id: first.node_id,
            target: first.target,
            kind: AgentEventKind::Replied {
                message_id: id(6),
                text: "first request".to_owned().try_into().expect("text")
            },
        }
    );
    let followup = send(11);
    let serialized = serde_json::to_vec(&followup).expect("encode wire command");
    let decoded = serde_json::from_slice(&serialized).expect("decode wire command");
    let reply = execute(&mut agent, &decoded).await.expect("follow up");
    assert_eq!(reply.target, first.target);
    assert_eq!(
        reply.kind,
        AgentEventKind::Replied {
            message_id: id(11),
            text: "follow-up".to_owned().try_into().expect("text")
        }
    );
    let session = agent.session(id(4)).expect("same session");
    assert_eq!(session.target(), first.target);
    assert_eq!(session.workspace_id(), id(5));
    assert_eq!(
        session.messages(),
        &[message(6, "first request"), message(11, "follow-up")]
    );
    assert!(!session.is_cancelled());
}

#[tokio::test]
async fn successful_command_retries_do_not_repeat_effects() {
    let mut agent = fake();
    let initial = start();
    let original = execute(&mut agent, &initial).await.expect("start");
    assert_eq!(execute(&mut agent, &initial).await, Ok(original));
    let followup = send(11);
    let original = execute(&mut agent, &followup).await.expect("send");
    assert_eq!(execute(&mut agent, &followup).await, Ok(original));
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 2);

    let cancel = command(12, AgentAction::Cancel);
    let cancelled = execute(&mut agent, &cancel).await.expect("cancel");
    assert_eq!(cancelled.kind, AgentEventKind::Cancelled);
    assert_eq!(execute(&mut agent, &cancel).await, Ok(cancelled));
    assert!(agent.session(id(4)).expect("session").is_cancelled());
    // A completed send may be replayed after cancellation, but cannot mutate it.
    assert!(execute(&mut agent, &followup).await.is_ok());
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 2);
}

#[tokio::test]
async fn reused_command_ids_cannot_change_the_payload() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    let snapshot = agent.session(id(4)).cloned();
    assert_eq!(
        execute(&mut agent, &send(10)).await,
        Err(AgentError::CommandConflict)
    );
    assert_eq!(agent.session(id(4)).cloned(), snapshot);
}

#[tokio::test]
async fn rejects_other_tenants_and_computers_before_reading_receipts() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    for change_group in [true, false] {
        let mut invalid = start();
        if change_group {
            invalid.group_id = id(100);
        } else {
            invalid.node_id = id(100);
        }
        assert_eq!(
            execute(&mut agent, &invalid).await,
            Err(AgentError::WrongRecipient)
        );
    }
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 1);
}

#[tokio::test]
async fn rejects_missing_sessions_and_duplicate_starts() {
    let mut agent = fake();
    assert_eq!(
        execute(&mut agent, &send(11)).await,
        Err(AgentError::SessionNotFound)
    );
    assert_eq!(
        execute(&mut agent, &command(12, AgentAction::Cancel)).await,
        Err(AgentError::SessionNotFound)
    );
    execute(&mut agent, &start()).await.expect("start");
    let mut duplicate = start();
    duplicate.command_id = id(13);
    assert_eq!(
        execute(&mut agent, &duplicate).await,
        Err(AgentError::SessionExists)
    );
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 1);
}

#[tokio::test]
async fn rejects_wrong_tasks_and_both_stale_and_future_epochs() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    let mut wrong_task = send(11);
    wrong_task.target.task_id = id(100);
    assert_eq!(
        execute(&mut agent, &wrong_task).await,
        Err(AgentError::WrongTask)
    );
    for epoch in [1, 3] {
        let mut stale = send(11);
        stale.target.execution_epoch = Epoch::try_from(epoch).expect("epoch");
        assert_eq!(
            execute(&mut agent, &stale).await,
            Err(AgentError::EpochMismatch {
                expected: Epoch::try_from(2).expect("expected"),
                received: stale.target.execution_epoch,
            })
        );
    }
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 1);
}

#[tokio::test]
async fn new_command_cannot_append_the_same_message_again() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    let duplicate = command(
        11,
        AgentAction::Send {
            message: message(6, "different content"),
        },
    );
    assert_eq!(
        execute(&mut agent, &duplicate).await,
        Err(AgentError::DuplicateMessage)
    );
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 1);
}

#[tokio::test]
async fn cancellation_stops_new_messages_and_is_safe_to_repeat() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    execute(&mut agent, &command(11, AgentAction::Cancel))
        .await
        .expect("cancel");
    assert_eq!(
        execute(&mut agent, &send(12)).await,
        Err(AgentError::SessionCancelled)
    );
    assert_eq!(
        execute(&mut agent, &command(13, AgentAction::Cancel))
            .await
            .expect("already cancelled")
            .kind,
        AgentEventKind::Cancelled
    );
}

#[tokio::test]
async fn injected_start_failure_has_no_effect_and_can_be_retried() {
    let mut agent = fake();
    agent.fail_next_command();
    assert_eq!(
        execute(&mut agent, &start()).await,
        Err(AgentError::InjectedFailure)
    );
    assert!(agent.session(id(4)).is_none());
    execute(&mut agent, &start()).await.expect("retry succeeds");
    assert_eq!(agent.session(id(4)).expect("session").messages().len(), 1);
}

#[tokio::test]
async fn injected_send_and_cancel_failures_are_atomic() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    for action in [send(11), command(12, AgentAction::Cancel)] {
        let before = agent.session(id(4)).cloned();
        agent.fail_next_command();
        assert_eq!(
            execute(&mut agent, &action).await,
            Err(AgentError::InjectedFailure)
        );
        assert_eq!(agent.session(id(4)).cloned(), before);
        execute(&mut agent, &action).await.expect("retry succeeds");
    }
    assert!(agent.session(id(4)).expect("session").is_cancelled());
}

#[tokio::test]
async fn rejected_commands_and_replays_do_not_consume_a_failure_injection() {
    let mut agent = fake();
    execute(&mut agent, &start()).await.expect("start");
    agent.fail_next_command();
    execute(&mut agent, &start()).await.expect("replay");
    let mut bad = send(11);
    bad.target.session_id = id(99);
    assert_eq!(
        execute(&mut agent, &bad).await,
        Err(AgentError::SessionNotFound)
    );
    assert_eq!(
        execute(&mut agent, &send(11)).await,
        Err(AgentError::InjectedFailure)
    );
    execute(&mut agent, &send(11)).await.expect("retry");
}

#[tokio::test]
async fn sessions_are_isolated_and_results_are_deterministic() {
    let mut first = fake();
    let mut second = fake();
    let mut other = start();
    other.command_id = id(20);
    other.target.session_id = id(21);
    other.target.task_id = id(22);
    for action in [start(), other, send(11), command(12, AgentAction::Cancel)] {
        assert_eq!(
            execute(&mut first, &action).await,
            execute(&mut second, &action).await
        );
    }
    assert_eq!(first.session(id(4)), second.session(id(4)));
    let isolated = first.session(id(21)).expect("other session");
    assert!(!isolated.is_cancelled());
    assert_eq!(isolated.messages().len(), 1);
    assert_ne!(
        isolated.target().task_id,
        first.session(id(4)).expect("first").target().task_id
    );
}

#[tokio::test]
async fn a_replacement_caller_can_continue_the_existing_specialist() {
    let mut runtime_agent = fake();
    let original_epoch = start().target.execution_epoch;
    let mut task = TaskState::restore(
        orchestrator_protocol::task::TaskStatus::Queued,
        original_epoch,
    )
    .transition(TaskAction::Begin)
    .expect("begin");
    // The specialist stays owned by the runtime, not by either supervisor handle.
    execute(&mut runtime_agent, &start())
        .await
        .expect("first supervisor starts");
    task = task
        .transition(TaskAction::RequestInput)
        .expect("await follow-up");
    let original_session: SessionId = id(4);
    let followup_command: CommandId = id(11);
    let result = execute(&mut runtime_agent, &send(11))
        .await
        .expect("replacement supervisor sends");
    task = task
        .transition(TaskAction::Resume)
        .expect("continue same execution");
    assert_eq!(result.command_id, followup_command);
    assert_eq!(result.target.session_id, original_session);
    assert_eq!(task.execution_epoch(), original_epoch);
    assert_eq!(
        runtime_agent
            .session(original_session)
            .expect("surviving session")
            .messages()
            .len(),
        2
    );
}

#[test]
fn adapter_errors_are_explicit_and_descriptive() {
    let errors = [
        (
            AgentError::WrongRecipient,
            "command is outside this agent's group or node",
        ),
        (
            AgentError::CommandConflict,
            "command ID was reused with different content",
        ),
        (AgentError::SessionExists, "session already exists"),
        (AgentError::SessionNotFound, "session does not exist"),
        (AgentError::WrongTask, "session belongs to a different task"),
        (
            AgentError::EpochMismatch {
                expected: Epoch::INITIAL,
                received: Epoch::INITIAL.next().expect("next"),
            },
            "execution epoch mismatch: expected 1, received 2",
        ),
        (AgentError::SessionCancelled, "session is cancelled"),
        (
            AgentError::DuplicateMessage,
            "message already exists in this session",
        ),
        (
            AgentError::InjectedFailure,
            "injected failure before execution",
        ),
    ];
    for (error, expected) in errors {
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_none());
    }
}
