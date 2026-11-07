//! Deterministic echo agent for contract, routing, and failure tests.

mod error;

pub use error::FakeAgentError;

use std::collections::BTreeMap;

use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind, Message, SessionTarget},
    ids::{CommandId, GroupId, NodeId, SessionId, WorkspaceId},
};

use crate::Agent;
use FakeAgentError as AgentError;

/// Read-only record of one fake specialist's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSession {
    target: SessionTarget,
    workspace_id: WorkspaceId,
    messages: Vec<Message>,
    cancelled: bool,
}

impl FakeSession {
    /// Stable task, session, and execution identity.
    #[must_use]
    pub const fn target(&self) -> SessionTarget {
        self.target
    }

    /// Workspace assigned at creation.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Accepted messages, in execution order.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Whether this specialist confirmed cancellation.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

/// In-memory echo sessions and successful command receipts for one node.
///
/// Replaying identical successful commands returns the original result. A new
/// command ID cannot reuse a message ID. Failures do not append messages or store
/// receipts. The fake uses no clocks, randomness, external services, or files.
#[derive(Debug)]
pub struct FakeAgent {
    group_id: GroupId,
    node_id: NodeId,
    sessions: BTreeMap<SessionId, FakeSession>,
    receipts: BTreeMap<CommandId, (AgentCommand, AgentEvent)>,
    fail_next: bool,
}

impl FakeAgent {
    /// Bind this fake to exactly one group and node.
    #[must_use]
    pub const fn new(group_id: GroupId, node_id: NodeId) -> Self {
        Self {
            group_id,
            node_id,
            sessions: BTreeMap::new(),
            receipts: BTreeMap::new(),
            fail_next: false,
        }
    }

    /// Inspect a session without granting mutation access.
    #[must_use]
    pub fn session(&self, id: SessionId) -> Option<&FakeSession> {
        self.sessions.get(&id)
    }

    /// Fail the next valid, not-yet-applied command before mutating any session.
    /// Retries of completed commands and invalid commands do not consume this flag.
    pub fn fail_next_command(&mut self) {
        self.fail_next = true;
    }

    fn existing_session(&self, target: SessionTarget) -> Result<FakeSession, AgentError> {
        let session = self
            .sessions
            .get(&target.session_id)
            .ok_or(AgentError::SessionNotFound)?;
        if session.target.task_id != target.task_id {
            return Err(AgentError::WrongTask);
        }
        if session.target.execution_epoch != target.execution_epoch {
            return Err(AgentError::EpochMismatch {
                expected: session.target.execution_epoch,
                received: target.execution_epoch,
            });
        }
        Ok(session.clone())
    }

    // Stage a copy so validation and injected failures cannot partially mutate
    // a session. Copying small test histories keeps the fake's transaction simple.
    fn prepare(&self, command: &AgentCommand) -> Result<(FakeSession, AgentEventKind), AgentError> {
        match &command.action {
            AgentAction::Start {
                workspace_id,
                message,
            } => {
                if self.sessions.contains_key(&command.target.session_id) {
                    return Err(AgentError::SessionExists);
                }
                Ok((
                    FakeSession {
                        target: command.target,
                        workspace_id: *workspace_id,
                        messages: vec![message.clone()],
                        cancelled: false,
                    },
                    AgentEventKind::Replied {
                        message_id: message.id,
                        text: message.text.clone(),
                    },
                ))
            }
            AgentAction::Send { message } => {
                let mut session = self.existing_session(command.target)?;
                if session.cancelled {
                    return Err(AgentError::SessionCancelled);
                }
                if session
                    .messages
                    .iter()
                    .any(|existing| existing.id == message.id)
                {
                    return Err(AgentError::DuplicateMessage);
                }
                session.messages.push(message.clone());
                Ok((
                    session,
                    AgentEventKind::Replied {
                        message_id: message.id,
                        text: message.text.clone(),
                    },
                ))
            }
            AgentAction::Cancel => {
                let mut session = self.existing_session(command.target)?;
                session.cancelled = true;
                Ok((session, AgentEventKind::Cancelled))
            }
        }
    }
}

impl Agent for FakeAgent {
    type Error = FakeAgentError;

    async fn execute(&mut self, command: &AgentCommand) -> Result<AgentEvent, AgentError> {
        if command.group_id != self.group_id || command.node_id != self.node_id {
            return Err(AgentError::WrongRecipient);
        }
        if let Some((original, event)) = self.receipts.get(&command.command_id) {
            return if original == command {
                Ok(event.clone())
            } else {
                Err(AgentError::CommandConflict)
            };
        }
        let (session, kind) = self.prepare(command)?;
        if std::mem::take(&mut self.fail_next) {
            return Err(AgentError::InjectedFailure);
        }
        let event = AgentEvent {
            version: command.version,
            command_id: command.command_id,
            group_id: command.group_id,
            node_id: command.node_id,
            target: command.target,
            kind,
        };
        self.sessions.insert(command.target.session_id, session);
        self.receipts
            .insert(command.command_id, (command.clone(), event.clone()));
        Ok(event)
    }
}
