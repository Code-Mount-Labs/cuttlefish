//! Goal, delegation and capability-learning contracts. Plans are data, never code.
use crate::{
    capabilities::Capability,
    ids::{CommandId, GoalId, NodeId, ReleaseId, TaskId, ThreadId},
    routing::{DispatchView, RouteRequest},
    values::MessageText,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A durable goal owned by one peer super agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalSpec {
    /// Goal identity, stable across retries.
    pub goal_id: GoalId,
    /// User conversation, distinct from child execution threads.
    pub thread_id: ThreadId,
    /// Requested outcome.
    pub objective: MessageText,
    /// Evidence required before completion.
    pub acceptance: MessageText,
}
/// A planned specialist task, with dependencies and a stable dispatch request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    /// Start request; its task, session and command IDs must be unique in the plan.
    pub request: RouteRequest,
    /// Tasks that must be verified before this task can start.
    pub dependencies: BTreeSet<TaskId>,
    /// Observable acceptance criteria for this task.
    pub acceptance: MessageText,
}
/// Goal lifecycle is distinct from individual command completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Planning or executing.
    Active,
    /// New delegation is paused; running work is not implicitly cancelled.
    Paused,
    /// All tasks were verified and a final result was recorded.
    Completed,
}
/// Measurable progress, separate from worker heartbeats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    /// Completed units; never greater than total.
    pub completed: u32,
    /// Fixed unit count for this execution.
    pub total: u32,
    /// Human-readable progress or blocker.
    pub note: MessageText,
}
/// Router-visible state of one child execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    /// Immutable work definition after dispatch begins.
    pub spec: TaskSpec,
    /// Durable dispatch intent; retries retain the original command identity.
    pub dispatching: bool,
    /// Latest authoritative execution receipt.
    pub execution: Option<DispatchView>,
    /// Most recent measured progress.
    pub progress: Option<Progress>,
    /// Verification evidence from the goal owner.
    pub verification: Option<MessageText>,
}
/// A directed discussion entry. Content is untrusted data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discussion {
    /// Authenticated author.
    pub author: NodeId,
    /// Optional addressee; absent means all participants.
    pub recipient: Option<NodeId>,
    /// Message, question, review or recorded decision.
    pub text: MessageText,
}
/// Versioned, organization-managed capability recipe or harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRelease {
    /// Immutable version identity; changes require a new release.
    pub release_id: ReleaseId,
    /// Human-readable name and version.
    pub name: MessageText,
    /// Capability supplied when installation and evaluation succeed.
    pub capability: Capability,
    /// Optional predecessor for an improved harness.
    pub supersedes: Option<ReleaseId>,
    /// Provenance, pinned artifact digest, setup steps and declared permissions.
    pub recipe: MessageText,
    /// Harness instructions. These cannot grant extra tool permissions.
    pub instructions: MessageText,
    /// Tests and acceptance criteria required for activation.
    pub evaluation: MessageText,
}

/// Reviewed capability deployment, reusable across goals in the same organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledCapability {
    /// Target peer.
    pub node_id: NodeId,
    /// Active immutable release.
    pub release: CapabilityRelease,
    /// Evidence reviewed at activation.
    pub evidence: MessageText,
}
/// Learning lifecycle. An agent cannot approve its own acquisition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LearningStatus {
    /// Awaiting a human operator's decision.
    Proposed {},
    /// Operator approved this exact immutable release for this node and goal.
    Approved {},
    /// Installation or evaluation was attempted; no capability is granted yet.
    Evaluated {
        /// Whether acceptance tests passed.
        passed: bool,
        /// Setup and test evidence.
        evidence: MessageText,
    },
    /// Operator accepted the evidence and activated the capability.
    Active {},
    /// Rejected acquisition or revoked capability.
    Rejected {
        /// Operator explanation.
        reason: MessageText,
    },
}
/// A node's proposed use of an immutable capability release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Learning {
    /// Goal task requiring the capability.
    pub task_id: TaskId,
    /// Installation/evaluation destination.
    pub node_id: NodeId,
    /// Exact reviewed package, copied from the managed catalog.
    pub release: CapabilityRelease,
    /// Why this capability is needed.
    pub rationale: MessageText,
    /// Persisted authorization and evaluation state.
    pub status: LearningStatus,
}
/// Full, bounded collaboration snapshot; executions are refreshed from dispatch storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalView {
    /// Original goal.
    pub spec: GoalSpec,
    /// Accountable peer, not a globally privileged agent.
    pub owner: NodeId,
    /// Monotonic revision for optimistic concurrency.
    pub revision: u64,
    /// Goal state.
    pub status: GoalStatus,
    /// Planned child work.
    pub tasks: Vec<TaskView>,
    /// Bounded discussions; full mutation history is retained separately.
    pub discussion: Vec<Discussion>,
    /// Acquisition and harness evaluation requests.
    pub learning: Vec<Learning>,
    /// Final evidence-backed answer.
    pub result: Option<MessageText>,
}
/// Idempotent, revision-checked mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalCommand {
    /// Operation identity, unchanged when retrying a lost response.
    pub command_id: CommandId,
    /// Revision observed by the decision maker.
    pub expected_revision: u64,
    /// Requested change.
    pub action: GoalAction,
}
/// Super-agent operations. Human approvals deliberately use a separate boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GoalAction {
    /// Add tasks. Existing work cannot be silently rewritten.
    Plan {
        /// At most 64 tasks in the whole goal.
        tasks: Vec<TaskSpec>,
    },
    /// Reserve a ready child for idempotent dispatch.
    Delegate {
        /// Child task identity.
        task_id: TaskId,
    },
    /// Publish measured progress as the assigned executor.
    Progress {
        /// Child task identity.
        task_id: TaskId,
        /// Units and summary.
        progress: Progress,
    },
    /// Discuss work or record a decision.
    Discuss {
        /// Optional addressed participant.
        recipient: Option<NodeId>,
        /// Message content.
        text: MessageText,
    },
    /// Accept a successful execution against its criteria.
    Verify {
        /// Child task identity.
        task_id: TaskId,
        /// Evidence supporting acceptance.
        evidence: MessageText,
    },
    /// Pause new dispatch; existing processes continue.
    Pause {},
    /// Resume planning and dispatch.
    Resume {},
    /// Complete only after every task has been verified.
    Finish {
        /// Final answer and evidence.
        result: MessageText,
    },
    /// Request acquisition from the managed catalog.
    Learn {
        /// Task missing a capability.
        task_id: TaskId,
        /// Destination node.
        node_id: NodeId,
        /// Managed catalog version.
        release_id: ReleaseId,
        /// Reason for acquisition.
        rationale: MessageText,
    },
    /// Record installation/evaluation evidence without self-activating.
    Evaluate {
        /// Evaluated catalog release.
        release_id: ReleaseId,
        /// Test outcome.
        passed: bool,
        /// Reproducible evaluation evidence.
        evidence: MessageText,
    },
    /// Propose an already learned release for additional nodes; each needs approval.
    Replicate {
        /// Existing active release.
        release_id: ReleaseId,
        /// Selected peers, never the whole group implicitly.
        nodes: BTreeSet<NodeId>,
        /// Explanation presented to the operator.
        rationale: MessageText,
    },
}
/// Human-only acquisition and activation decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LearningDecision {
    /// Approve the immutable recipe for installation and evaluation.
    Approve {},
    /// Activate after a passing evaluation has been reviewed.
    Activate {},
    /// Reject or revoke.
    Reject {
        /// Operator explanation.
        reason: MessageText,
    },
}
