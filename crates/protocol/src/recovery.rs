//! Explicit, reviewed continuation of an interrupted provider turn.
use crate::{
    agent::AgentCommand,
    execution::LocalProvider,
    ids::{CommandId, SessionId},
};
use serde::{Deserialize, Serialize};
/// A durable recovery stage; inspection never executes the original request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    /// Waiting for the owning node to inspect local evidence.
    Inspecting,
    /// A checkpoint is available for human review.
    Ready,
    /// Evidence is insufficient; no execution is permitted.
    Blocked,
    /// The reviewed checkpoint was explicitly approved.
    Approved,
    /// Continuation was durably armed on the owning node.
    Resumed,
}
/// Bounded, non-secret description of saved work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// Exact provider whose saved session will be reused.
    pub provider: LocalProvider,
    /// Stable Cuttlefish specialist session.
    pub session_id: SessionId,
    /// Number of regular files inspected, excluding generated dependency folders.
    pub files: u32,
    /// Most recent public progress summary, if present.
    pub progress: Option<String>,
    /// Digest of local binding, task, configuration and workspace file contents.
    pub digest: String,
}
/// Public state of a recovery request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryView {
    /// Unique inspection identity, retained across retries.
    pub id: CommandId,
    /// Interrupted command; its input is never automatically replayed.
    pub command_id: CommandId,
    /// Current recovery stage.
    pub state: RecoveryState,
    /// Checkpoint approved by the user, when available.
    pub checkpoint: Option<Checkpoint>,
    /// Controlled explanation for blocked recovery.
    pub detail: Option<String>,
}
/// Work sent only to the original execution node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryJob {
    /// Review state.
    pub view: RecoveryView,
    /// Exact saved execution address and original task.
    pub command: AgentCommand,
}
/// Authenticated user operations. Approval names the exact inspected snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryAction {
    /// Inspect or refresh saved state without running a provider.
    Inspect {
        /// Idempotent inspection identity.
        id: CommandId,
        /// Interrupted turn in the selected conversation.
        command_id: CommandId,
    },
    /// Continue only the reviewed checkpoint.
    Continue {
        /// Inspection being approved.
        id: CommandId,
        /// Exact reviewed snapshot digest.
        digest: String,
    },
}
