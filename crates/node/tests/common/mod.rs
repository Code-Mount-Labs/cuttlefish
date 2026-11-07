#![allow(dead_code, clippy::expect_used)]

use std::{fmt::Debug, str::FromStr};

use orchestrator_node::{
    journal::Journal,
    model::{CommandOutcome, CommandStatus, NodeIdentity, SessionRecord},
};
use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind, Message, SessionTarget},
    values::{Epoch, ProtocolVersion},
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tempfile::TempDir;
use uuid::Uuid;

/// Controllable adapter boundary: normal fake behavior, plus a held operation.
#[derive(Debug)]
pub struct TestAgent {
    inner: orchestrator_agents::fake::FakeAgent,
    pub hold: Option<(
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    )>,
}

impl TestAgent {
    pub fn new(
        group: orchestrator_protocol::ids::GroupId,
        node: orchestrator_protocol::ids::NodeId,
    ) -> Self {
        Self {
            inner: orchestrator_agents::fake::FakeAgent::new(group, node),
            hold: None,
        }
    }
    pub fn fail_next_command(&mut self) {
        self.inner.fail_next_command();
    }
}

impl orchestrator_agents::Agent for TestAgent {
    type Error = orchestrator_agents::fake::FakeAgentError;
    async fn execute(&mut self, command: &AgentCommand) -> Result<AgentEvent, Self::Error> {
        if let Some((entered, release)) = self.hold.take() {
            entered.notify_one();
            release.notified().await;
        }
        self.inner.execute(command).await
    }
}

pub fn id<T: FromStr>(value: u128) -> T
where
    T::Err: Debug,
{
    format!("{value:032x}").parse().expect("fixture identity")
}

pub fn identity() -> NodeIdentity {
    NodeIdentity {
        group_id: id(1),
        node_id: id(2),
    }
}

pub fn instance() -> Uuid {
    Uuid::from_u128(900)
}

pub fn message(value: u128) -> Message {
    Message {
        attachments: Vec::new(),
        id: id(value),
        text: format!("message {value}").try_into().expect("fixture text"),
    }
}

pub fn start() -> AgentCommand {
    AgentCommand {
        version: ProtocolVersion::V1,
        command_id: id(6),
        group_id: id(1),
        node_id: id(2),
        target: SessionTarget {
            task_id: id(3),
            session_id: id(4),
            execution_epoch: Epoch::INITIAL,
        },
        action: AgentAction::Start {
            workspace_id: id(5),
            message: message(7),
        },
    }
}

pub fn send(value: u128) -> AgentCommand {
    AgentCommand {
        command_id: id(value),
        action: AgentAction::Send {
            message: message(value),
        },
        ..start()
    }
}

pub fn cancel(value: u128) -> AgentCommand {
    AgentCommand {
        command_id: id(value),
        action: AgentAction::Cancel,
        ..start()
    }
}

pub fn outcome(command: &AgentCommand) -> CommandOutcome {
    CommandOutcome::Completed {
        event: Box::new(AgentEvent {
            artifacts: Vec::new(),
            version: command.version,
            command_id: command.command_id,
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            kind: match &command.action {
                AgentAction::Start { message, .. } | AgentAction::Send { message } => {
                    AgentEventKind::Replied {
                        message_id: message.id,
                        text: message.text.clone(),
                    }
                }
                AgentAction::Cancel => AgentEventKind::Cancelled,
            },
        }),
    }
}

pub struct Fixture {
    pub journal: Journal,
    pub temp: TempDir,
}

impl Fixture {
    pub async fn session(&self) -> SessionRecord {
        self.journal
            .session(id(4))
            .await
            .expect("session query")
            .expect("saved session")
    }

    pub async fn status(&self, number: u128) -> CommandStatus {
        self.journal
            .command(id(number))
            .await
            .expect("command query")
            .expect("saved command")
            .status
    }

    pub async fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary directory");
        let journal = Journal::open(&temp.path().join("node.sqlite"), identity())
            .await
            .expect("journal");
        journal.mount(id(5), temp.path()).await.expect("mount");
        Self { journal, temp }
    }

    pub async fn accepted(&self) {
        self.journal
            .accept(&start(), instance())
            .await
            .expect("accept");
    }

    pub async fn registered(&self) {
        self.accepted().await;
        self.journal
            .register(id(4), &instance().to_string(), 42)
            .await
            .expect("register");
    }

    pub async fn completed(&self) {
        self.registered().await;
        self.journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("claim")
            .expect("work");
        self.journal
            .complete(id(6), &instance().to_string(), &outcome(&start()))
            .await
            .expect("complete");
    }

    pub async fn raw(&self) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(self.journal.path()))
            .await
            .expect("test connection")
    }
}
