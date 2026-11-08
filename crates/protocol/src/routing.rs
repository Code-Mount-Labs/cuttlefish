//! Routing requests, immutable execution addresses, and durable delivery contracts.
use crate::{
    agent::{AgentCommand, Message, SessionTarget},
    capabilities::Capability,
    ids::{CommandId, NodeId, SessionId, TaskId, ThreadId, WorkspaceId},
    sync::{CommandOutcome, NodeProfile},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A caller's idempotent request within one conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    /// Never reuse this identity with different content.
    pub command_id: CommandId,
    /// Conversation whose execution affinity is retained durably.
    pub thread_id: ThreadId,
    /// Requested operation; routing cannot rewrite the message.
    pub action: RouteAction,
}

/// Explicit operations; a restart is never inferred from a missed heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteAction {
    /// Begin the thread's one specialist task.
    Start {
        /// Logical task.
        task_id: TaskId,
        /// Initial specialist identity.
        session_id: SessionId,
        /// Logical directory registration required on the destination.
        workspace_id: WorkspaceId,
        /// Hard constraints checked independently of the model.
        capabilities: BTreeSet<Capability>,
        /// Initial message.
        message: Message,
    },
    /// Continue the exact bound specialist.
    Send {
        /// Follow-up input.
        message: Message,
    },
    /// Ask the bound specialist to confirm stoppage.
    Cancel {},
    /// Explicit new execution, allowed only after confirmed cancellation.
    Restart {
        /// A fresh specialist identity, never the old session.
        session_id: SessionId,
        /// New work, not a replay of unknown old effects.
        message: Message,
    },
}

/// Durable thread ownership, separate from a router replica's lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Execution node.
    pub node_id: NodeId,
    /// Exact specialist and execution epoch.
    pub target: SessionTarget,
    /// Required logical workspace.
    pub workspace_id: WorkspaceId,
    /// Original task's capability constraints.
    pub capabilities: BTreeSet<Capability>,
}

/// Delivery state. Claimed work is never automatically moved to another node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Not yet handed to any executor.
    Queued,
    /// Delivery intent committed before returning the command to a node.
    Claimed,
    /// An authenticated result was committed.
    Completed,
}

/// Current view of one request, including its stable thread and execution address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchView {
    /// Original request.
    pub request: RouteRequest,
    /// Selected execution address and unchanged input.
    pub command: AgentCommand,
    /// Delivery progress, not a guarantee of external effects.
    pub state: DeliveryState,
    /// Result when known.
    pub outcome: Option<CommandOutcome>,
}

/// A prequalified node. The LLM may choose only among these candidates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Destination identity.
    pub node_id: NodeId,
    /// Declared capabilities and platform.
    pub profile: NodeProfile,
    /// Unreserved capacity after the latest executor poll.
    pub available_slots: i64,
}

/// Small, group-scoped, untrusted activity excerpt supplied as model data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    /// Source node.
    pub node_id: NodeId,
    /// Excerpt, never a routing instruction.
    pub summary: String,
}

/// Bounded context for selecting a destination. No credentials or local paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingContext {
    /// Bounded view of collaborative work already assigned in the group.
    pub active_work: Vec<GoalWork>,
    /// Exact user's request.
    pub request: RouteRequest,
    /// Fresh, capable, mounted nodes with free capacity.
    pub candidates: Vec<Candidate>,
    /// At most eight recent excerpts, each at most 1,000 characters.
    pub recent_activity: Vec<Activity>,
}

/// Concise collaboration context; complete goal records remain in the goal API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalWork {
    /// Parent goal.
    pub goal_id: crate::ids::GoalId,
    /// Child task.
    pub task_id: TaskId,
    /// Accountable super agent.
    pub owner: NodeId,
    /// Selected execution node, when assigned.
    pub executor: Option<NodeId>,
    /// Exact specialist session, when assigned.
    pub session_id: Option<SessionId>,
    /// Current delivery state, or planned/reserved.
    pub state: String,
    /// Progress or acceptance summary, bounded to 1,000 characters.
    pub summary: String,
}

/// A node's dispatch poll and durable acceptance acknowledgments.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchPoll {
    /// Currently registered workspace identities, not paths.
    pub workspaces: Vec<WorkspaceId>,
    /// Latest count, sampled after persisting the acknowledged commands.
    pub active_sessions: u32,
    /// Commands durably accepted into this node's journal.
    pub accepted: Vec<CommandId>,
}

/// One delivery at a time; responses may be lost and safely redelivered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchBatch {
    /// Explicit inspection or approved recovery on the original node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<crate::recovery::RecoveryJob>,
    /// Accepted IDs recorded by the server, in request order.
    pub acknowledged: Vec<CommandId>,
    /// Already marked claimed in `PostgreSQL` before transport begins.
    pub command: Option<AgentCommand>,
}
