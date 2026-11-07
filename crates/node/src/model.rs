//! Local journal records. These are not remote server API contracts.

use orchestrator_protocol::{
    agent::{AgentCommand, SessionTarget},
    ids::{GroupId, NodeId, WorkspaceId},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, types::Json};

/// The only group and computer permitted to use this journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeIdentity {
    /// Tenant identity.
    pub group_id: GroupId,
    /// Computer identity.
    pub node_id: NodeId,
}

/// Local process lifecycle, independent of a task's business outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Launch intent is durable; the worker has not registered yet.
    Starting,
    /// The original worker has registered and is sending heartbeats.
    Running,
    /// The original worker confirmed shutdown.
    Stopped,
    /// The worker exited unexpectedly or its heartbeat expired.
    Interrupted,
}

/// Durable command state. Uncertain commands are never automatically reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Saved before acknowledgment, awaiting its worker.
    Queued,
    /// Claimed by the original worker; effects might have occurred.
    Executing,
    /// A result was committed atomically with an outgoing event.
    Completed,
    /// The adapter rejected the command with a known outcome.
    Rejected,
    /// Execution or launch was interrupted; reconciliation is required.
    Uncertain,
}

pub use orchestrator_protocol::sync::CommandOutcome;

/// Persisted receipt returned on both initial submission and redelivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct CommandRecord {
    /// Local FIFO ordering, allocated in the acceptance transaction.
    pub sequence: i64,
    /// Complete request used to detect ID/payload conflicts.
    pub request: Json<AgentCommand>,
    /// Current processing state.
    pub status: CommandStatus,
    /// Present only for known outcomes.
    pub outcome: Option<Json<CommandOutcome>>,
}

/// Observable process identity. PIDs are informational, never used to kill an
/// unknown process after restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct SessionRecord {
    /// Stable specialist execution address.
    pub target: Json<SessionTarget>,
    /// Node-local workspace registration.
    pub workspace_id: String,
    /// One-time launch identity; a second worker cannot claim it.
    pub instance_id: String,
    /// Last confirmed process state.
    pub state: SessionStatus,
    /// Original worker process ID.
    pub pid: Option<i64>,
    /// UTC milliseconds of the last registration or heartbeat.
    pub heartbeat_at: i64,
    /// Local operator requested shutdown.
    pub stop_requested: bool,
    /// Diagnostic reason for an unexpected exit.
    pub last_error: Option<String>,
}

/// Result awaiting delivery to the separate coordination service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct PendingEvent {
    /// Stable journal event identity.
    pub sequence: i64,
    /// Command identity whose outcome this event records.
    pub command_id: String,
    /// Result saved in the same transaction as its receipt.
    pub outcome: Json<CommandOutcome>,
}

/// Immutable information needed to spawn a specialist exactly once.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Original command and execution address.
    pub command: AgentCommand,
    /// One-time launch token persisted before the spawn attempt.
    pub instance_id: String,
    /// Canonical allowed working directory.
    pub directory: std::path::PathBuf,
}

/// Result of acceptance; only a newly created session has a launch intent.
#[derive(Debug)]
pub struct Acceptance {
    /// Durable command record, possibly replayed from a prior supervisor.
    pub record: CommandRecord,
    /// A spawn is permitted only for this newly accepted request.
    pub launch: Option<LaunchSpec>,
}

/// A local directory registration supplied by the node operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMount {
    /// Logical workspace identity shared with the server.
    pub workspace_id: WorkspaceId,
    /// Local directory; it is never sent to other computers.
    pub path: std::path::PathBuf,
}
