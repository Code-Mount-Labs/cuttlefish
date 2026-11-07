//! Hosted group registry contracts, independent of the router's execution API.

use crate::{
    ids::{GroupId, InstallationId},
    sync::NodeToken,
    values::MessageText,
};
use serde::{Deserialize, Serialize};

/// Responsibilities selected when installing a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationRole {
    /// The default: one router and one local agent.
    RouterAgent,
    /// Only the group's routing service.
    Router,
    /// Only an execution agent, requiring an existing ready router.
    Agent,
}
impl InstallationRole {
    /// Whether this installation claims the group's one router slot.
    #[must_use]
    pub const fn has_router(self) -> bool {
        matches!(self, Self::Router | Self::RouterAgent)
    }
    /// Whether this installation runs an execution agent.
    #[must_use]
    pub const fn has_agent(self) -> bool {
        matches!(self, Self::Agent | Self::RouterAgent)
    }
}

/// Registration saved locally before the installer makes any network request.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    /// Group created on the website.
    pub group_id: GroupId,
    /// Stable identity across installer retries.
    pub installation_id: InstallationId,
    /// Chosen installation topology.
    pub role: InstallationRole,
    /// Display name, never used for authorization.
    pub name: MessageText,
    /// Reachable router origin; absent on agent-only installations.
    pub router_url: Option<String>,
    /// Per-installation credential, generated locally and never returned by listings.
    #[serde(with = "wire_token")]
    pub credential: NodeToken,
    /// One-use invitation obtained from the group owner.
    #[serde(with = "wire_token")]
    pub invitation: NodeToken,
}
impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Registration { credentials: [redacted] }")
    }
}

/// Safe installation listing for web and mobile clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationView {
    /// Stable registry identity.
    pub installation_id: InstallationId,
    /// Group scope.
    pub group_id: GroupId,
    /// Router, agent, or both.
    pub role: InstallationRole,
    /// Human-readable name.
    pub name: String,
    /// Router origin for outbound agent connections.
    pub router_url: Option<String>,
    /// Optional browser URL for router administration.
    pub router_dashboard_url: Option<String>,
    /// Optional browser URL for local agent administration.
    pub agent_dashboard_url: Option<String>,
    /// Fresh heartbeat from a ready installation.
    pub online: bool,
}

/// A running installation advertises only its own status and dashboard addresses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationHeartbeat {
    /// Whether the selected services have successfully started.
    pub ready: bool,
    /// Optional router dashboard address.
    pub router_dashboard_url: Option<String>,
    /// Optional agent dashboard address.
    pub agent_dashboard_url: Option<String>,
}

/// Explicit credential serialization for registration only. Ordinary node tokens
/// deliberately have no blanket Serialize implementation.
mod wire_token {
    use super::NodeToken;
    use serde::{Deserialize, Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(
        value: &NodeToken,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.expose())
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<NodeToken, D::Error> {
        NodeToken::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
