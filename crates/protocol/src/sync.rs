//! Node authentication and bounded event synchronization contracts.

use crate::{
    agent::{AgentAction, AgentCommand, AgentEvent, AgentEventKind},
    capabilities::{Capability, Platform},
    ids::{GroupId, NodeId},
    values::ProtocolVersion,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};
use thiserror::Error;

/// Maximum events in one synchronization transaction.
pub const BATCH_SIZE: usize = 4;

/// Opaque high-entropy credential. Debug output never exposes the value.
#[derive(Clone)]
pub struct NodeToken(String);

/// Invalid credential syntax; never contains the supplied secret.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("node token must contain exactly 64 lowercase hexadecimal characters")]
pub struct InvalidToken;

impl TryFrom<String> for NodeToken {
    type Error = InvalidToken;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidToken);
        }
        Ok(Self(value))
    }
}

impl NodeToken {
    /// Reveal only when constructing an authenticated request or credential hash.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NodeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NodeToken([REDACTED])")
    }
}

/// Durable specialist outcome, shared without exposing local filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandOutcome {
    /// Completed specialist operation.
    Completed {
        /// Correlated result.
        event: Box<AgentEvent>,
    },
    /// Known adapter rejection, distinct from an uncertain operation.
    Rejected {
        /// Diagnostic text.
        reason: String,
    },
}

/// Operator-configured routing capabilities, not an assertion of authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeProfile {
    /// Native platform.
    pub platform: Platform,
    /// Explicitly supported capabilities.
    pub capabilities: BTreeSet<Capability>,
    /// Configured concurrent session capacity.
    pub capacity: u16,
}

/// One immutable local outbox record with its original execution address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncEvent {
    /// Positive, durable local event sequence; never reused within a node identity.
    pub sequence: i64,
    /// Original command, including task/session/epoch correlation.
    pub command: AgentCommand,
    /// Outcome committed alongside the local receipt.
    pub outcome: CommandOutcome,
}

/// An authenticated node heartbeat and bounded outbox batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncRequest {
    /// Wire version.
    pub version: ProtocolVersion,
    /// Claimed group must match the credential.
    pub group_id: GroupId,
    /// Claimed node must match the credential.
    pub node_id: NodeId,
    /// Capabilities supplied by the node operator.
    pub profile: NodeProfile,
    /// Active or unresolved sessions observed in the local journal.
    pub active_sessions: u32,
    /// Immutable records; empty batches still update presence.
    pub events: Vec<SyncEvent>,
}

/// Report is invalid; diagnostics never contain submitted messages or tokens.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct InvalidReport(pub &'static str);

impl SyncRequest {
    /// Check bounds and correlations before trusting this report.
    ///
    /// # Errors
    /// Rejects oversized batches, duplicate identities, and mismatched outcomes.
    pub fn validate(&self) -> Result<(), InvalidReport> {
        if self.events.len() > BATCH_SIZE
            || self.profile.capabilities.len() > 64
            || self.profile.capacity == 0
        {
            return Err(InvalidReport("report exceeds limits or has zero capacity"));
        }
        let mut sequences = BTreeSet::new();
        let mut commands = BTreeSet::new();
        for item in &self.events {
            if item.sequence <= 0
                || !sequences.insert(item.sequence)
                || !commands.insert(item.command.command_id)
            {
                return Err(InvalidReport(
                    "event identities must be positive and unique",
                ));
            }
            if item.command.group_id != self.group_id || item.command.node_id != self.node_id {
                return Err(InvalidReport("event belongs to a different node or group"));
            }
            match &item.outcome {
                CommandOutcome::Rejected { reason } => {
                    if reason.is_empty() || reason.len() > 65_536 {
                        return Err(InvalidReport("invalid rejection reason"));
                    }
                }
                CommandOutcome::Completed { event } => {
                    let command = &item.command;
                    let correlated = event.version == command.version
                        && event.command_id == command.command_id
                        && event.group_id == command.group_id
                        && event.node_id == command.node_id
                        && event.target == command.target;
                    let action_matches = match (&command.action, &event.kind) {
                        (
                            AgentAction::Start { message, .. } | AgentAction::Send { message },
                            AgentEventKind::Replied { message_id, .. },
                        ) => message.id == *message_id,
                        (AgentAction::Cancel, AgentEventKind::Cancelled) => true,
                        _ => false,
                    };
                    if !correlated || !action_matches {
                        return Err(InvalidReport("outcome does not match its command"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Acknowledgments are returned only after the complete transaction commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncResponse {
    /// Wire version.
    pub version: ProtocolVersion,
    /// Exactly the local sequences supplied in this request, in the same order.
    pub acknowledged: Vec<i64>,
}

/// Group-scoped routing context. Availability must consider the observation age.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeView {
    /// Peer node identity.
    pub node_id: NodeId,
    /// Most recent reported profile, absent until the first connection.
    pub profile: Option<NodeProfile>,
    /// Number of active or unresolved local sessions.
    pub active_sessions: i64,
    /// Server-observed heartbeat in UTC milliseconds, absent before first sync.
    pub last_seen_ms: Option<i64>,
}

/// A durable, group-scoped event. Treat its text as untrusted routing context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventView {
    /// Server cursor; use the last returned value for the next page.
    pub cursor: i64,
    /// Original local record.
    pub event: SyncEvent,
}
