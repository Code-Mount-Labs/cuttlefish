//! Remote-access contracts. Provider credentials never appear in directory views.
use serde::{Deserialize, Serialize};

/// Network exposure chosen by the installation administrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteChoice {
    /// Listen locally without maintaining a public tunnel.
    Local {},
    /// Use an HTTPS reverse proxy configured by the administrator.
    Existing {
        /// Externally reachable HTTPS origin, without a path.
        origin: String,
    },
    /// Request an installation-scoped address from the hosted platform.
    Managed {},
}

/// Desired origin update, replayable after an interrupted directory request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressUpdate {
    /// Monotonic installation-local revision; older updates must be rejected.
    pub revision: i64,
    /// Verified public address, or the installation's loopback address.
    pub origin: String,
}

/// An idempotent connector-credential change. Absence of a credential revokes access.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayMutation {
    /// Monotonically increasing installation-local configuration revision.
    pub revision: i64,
    /// A newly generated, locally persisted credential; never the enrollment token.
    pub credential: Option<String>,
}

/// Public connection parameters for one installation; contains no secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayLease {
    /// Identity of the authorized installation.
    pub installation_id: crate::ids::InstallationId,
    /// Changes whenever access is replaced or revoked.
    pub epoch: uuid::Uuid,
    /// Installation-scoped HTTPS base URL, without a trailing slash.
    pub public_url: String,
    /// Hosted issuer base URL, without a trailing slash.
    pub issuer: String,
}

/// Validate an HTTPS endpoint, permitting literal loopback HTTP for local setups.
/// Only the root or a canonical `/i/<installation UUID>` prefix is accepted.
/// User information, encoded paths, query strings and fragments are rejected.
#[must_use]
pub fn canonical_endpoint(value: &str) -> Option<String> {
    endpoint_url(value).map(|url| url.as_str().trim_end_matches('/').to_owned())
}

/// Parse a validated root or installation-scoped endpoint without reparsing it later.
#[must_use]
pub fn endpoint_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    let local = url
        .host_str()?
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback());
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['%', '\\'])
        || value.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    if url.path().contains("//") {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    if !path.is_empty() {
        let id = path.strip_prefix("/i/")?;
        let parsed = uuid::Uuid::parse_str(id).ok()?;
        if parsed.is_nil() || parsed.to_string() != id {
            return None;
        }
    }
    Some(url)
}

impl std::fmt::Debug for RelayMutation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayMutation")
            .field("revision", &self.revision)
            .field("credential", &"[redacted]")
            .finish()
    }
}
