//! Provider execution identity and human permission contracts. These do not
//! grant permission to install a capability or to change a requested action.

use crate::{
    agent::SessionTarget,
    capabilities::Capability,
    ids::{CommandId, GroupId, NodeId, PermissionId, ReviewerId},
    values::MessageText,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Immutable mapping from a Cuttlefish execution to one provider session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    /// Owning tenant.
    pub group_id: GroupId,
    /// Executing computer; bindings are not portable to another node.
    pub node_id: NodeId,
    /// Cuttlefish task, session and fencing epoch.
    pub target: SessionTarget,
    /// Configured provider identity, independent of capability selection.
    pub provider: Capability,
    /// Opaque provider session ID, never a user conversation ID.
    pub provider_session_id: MessageText,
    /// Whether the adapter can reconnect without starting a new execution.
    pub resumable: bool,
}

/// Exact proposed effect. Adapters must submit the same bytes to the provider;
/// editing even whitespace creates a different approval scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    /// Stable identity for redelivery of this request only.
    pub permission_id: PermissionId,
    /// Complete provider and execution scope.
    pub binding: ProviderBinding,
    /// Executing command that caused the request.
    pub command_id: CommandId,
    /// Provider's opaque callback/request identity.
    pub provider_request_id: MessageText,
    /// Tool name, arguments and relevant resource scope, as exact reviewed text.
    pub action: MessageText,
    /// UTC milliseconds, exclusive expiration boundary.
    pub expires_at_ms: i64,
}

impl PermissionRequest {
    /// SHA-256 over a versioned, length-prefixed encoding of every scope field.
    /// Length prefixes prevent ambiguous concatenations; no JSON ordering is involved.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"cuttlefish.runtime-permission.v1\0");
        for field in [
            self.permission_id.to_string(),
            self.binding.group_id.to_string(),
            self.binding.node_id.to_string(),
            self.binding.target.task_id.to_string(),
            self.binding.target.session_id.to_string(),
            self.binding.target.execution_epoch.to_string(),
            self.binding.provider.as_str().to_owned(),
            self.binding.provider_session_id.as_str().to_owned(),
            self.binding.resumable.to_string(),
            self.command_id.to_string(),
            self.provider_request_id.as_str().to_owned(),
            self.action.as_str().to_owned(),
            self.expires_at_ms.to_string(),
        ] {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field.as_bytes());
        }
        format!("{:x}", hash.finalize())
    }
}

/// A human decision applies to one exact request, not a provider or a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChoice {
    /// Permit the reviewed effect once.
    Allow,
    /// Refuse the reviewed effect.
    Deny,
}

/// Input from the authenticated review channel. Actor and time are server-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionDecision {
    /// Decision idempotency identity.
    pub decision_id: CommandId,
    /// Request being reviewed.
    pub permission_id: PermissionId,
    /// Exact digest displayed to the reviewer.
    pub request_digest: String,
    /// Human choice.
    pub choice: PermissionChoice,
}

/// Durable, authenticated decision history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionReview {
    /// Unmodified decision input.
    pub decision: PermissionDecision,
    /// Identity derived from the enrolled credential, never model output.
    pub reviewer_id: ReviewerId,
    /// Trusted journal clock at acceptance.
    pub decided_at_ms: i64,
}

/// Permission delivery is separate from completion of the containing command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    /// Waiting for an authenticated human decision.
    Waiting,
    /// Human decision saved; no provider reply has been attempted.
    Decided,
    /// Reply may have reached the provider. Never retry automatically.
    Dispatching,
    /// Provider confirmed receipt of the decision, not completion of the task.
    Delivered,
    /// Withdrawn before any reply was attempted. Grants no execution authority.
    Abandoned,
}

/// Readable durable state, including the original action and actor attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRecord {
    /// Immutable request.
    pub request: PermissionRequest,
    /// Explicit delivery phase.
    pub state: PermissionState,
    /// Present after a human decision has been committed.
    pub review: Option<PermissionReview>,
}
