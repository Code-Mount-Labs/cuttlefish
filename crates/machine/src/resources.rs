//! Durable database provisioning contracts. The backend must reconcile an exact
//! saved specification rather than create new credentials or volumes on retry.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// User's database selection; credentials are accepted only by the local API.
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseChoice {
    /// Connect an existing user-owned database.
    Existing {
        /// Connection string, never returned by status endpoints.
        url: String,
    },
    /// Provision `PostgreSQL` with local-only networking and durable storage.
    Managed {
        /// Local TCP port, allowing multiple independent installations.
        port: u16,
    },
    /// Keep the setup dashboard available without starting the router.
    Later,
}
impl std::fmt::Debug for DatabaseChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseChoice([credentials redacted])")
    }
}

/// Saved exact database configuration. This is private local state, not an API response.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseSpec {
    /// An externally administered database.
    Existing {
        /// Private connection string.
        url: String,
    },
    /// Locally provisioned database with stable identity and credentials.
    Managed {
        /// Exact installation-scoped container name.
        container: String,
        /// Exact installation-scoped volume name.
        volume: String,
        /// Local-only host port.
        port: u16,
        /// Stable generated database password, never regenerated on recovery.
        password: String,
        /// Bootstrap administrator secret, used only while provisioning.
        admin_password: String,
    },
}
impl std::fmt::Debug for DatabaseSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseSpec([credentials redacted])")
    }
}
impl DatabaseSpec {
    /// Obtain the private connection URL at the database boundary only.
    #[must_use]
    pub fn connection_url(&self) -> String {
        match self {
            Self::Existing { url } => url.clone(),
            Self::Managed { port, password, .. } => {
                format!("postgresql://cuttlefish:{password}@127.0.0.1:{port}/cuttlefish")
            }
        }
    }
}

/// Public setup state, excluding all database credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceStatus {
    /// Monotonic configuration revision.
    pub revision: i64,
    /// unconfigured, provisioning, ready, or failed.
    pub state: String,
    /// existing or managed, when selected.
    pub mode: Option<String>,
    /// Stable, redacted failure category for actionable UI messaging.
    pub error_code: Option<String>,
}

/// Object-safe future returned by resource backends.
pub type ResourceFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

/// Boundary for idempotent creation and connectivity checks. Tests can simulate
/// power loss without replacing the real journal or state machine.
pub trait DatabaseBackend: std::fmt::Debug + Send + Sync {
    /// Ensure the exact saved resource exists, then verify primary connectivity.
    fn reconcile<'a>(&'a self, spec: &'a DatabaseSpec) -> ResourceFuture<'a>;
}
