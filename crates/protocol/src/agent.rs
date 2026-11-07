//! Specialist commands and their deterministic responses.

use serde::{Deserialize, Serialize};

use crate::{
    ids::{CommandId, GroupId, MessageId, NodeId, SessionId, TaskId, WorkspaceId},
    values::{Epoch, MessageText, ProtocolVersion},
};

/// An input whose identity survives redelivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    /// Stable identity supplied by the application.
    pub id: MessageId,
    /// Original user or supervisor text.
    pub text: MessageText,
}

/// An execution address. Reassigning a supervisor does not change this epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionTarget {
    /// Logical task to which the session belongs.
    pub task_id: TaskId,
    /// Exact specialist session to resume.
    pub session_id: SessionId,
    /// Authority for this specialist execution, not supervisor ownership.
    pub execution_epoch: Epoch,
}

/// The only specialist mutations in milestone one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    /// Start a new session and deliver its first message.
    Start {
        /// Registered logical workspace; never a user-supplied local path.
        workspace_id: WorkspaceId,
        /// First message in the session.
        message: Message,
    },
    /// Deliver a follow-up to an existing session.
    Send {
        /// Message to append exactly once for this command.
        message: Message,
    },
    /// Stop the session; later sends must fail.
    Cancel,
}

/// A versioned command. The entire value is compared when its ID is retried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommand {
    /// Wire contract version.
    pub version: ProtocolVersion,
    /// Durable idempotency key.
    pub command_id: CommandId,
    /// Tenant boundary.
    pub group_id: GroupId,
    /// Computer authorized to execute this command.
    pub node_id: NodeId,
    /// Specialist identity and ownership version.
    pub target: SessionTarget,
    /// Typed action payload.
    pub action: AgentAction,
}

/// A completed command's result. Delivery acknowledgment is a separate concern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEventKind {
    /// The specialist produced a response to a particular input.
    Replied {
        /// Input that caused the response.
        message_id: MessageId,
        /// Response content.
        text: MessageText,
    },
    /// The specialist confirmed it has stopped.
    Cancelled,
}

/// A response correlated with its command and original execution address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvent {
    /// Wire contract version.
    pub version: ProtocolVersion,
    /// Command that produced this result.
    pub command_id: CommandId,
    /// Tenant boundary.
    pub group_id: GroupId,
    /// Executing computer.
    pub node_id: NodeId,
    /// Exact session and execution version.
    pub target: SessionTarget,
    /// Typed result payload.
    pub kind: AgentEventKind,
}
