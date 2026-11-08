//! Durable local node runtime. Specialists communicate through a local `SQLite`
//! journal and do not depend on a supervisor's pipes or process lifetime.

pub mod activity;
pub mod cli;
pub mod journal;
pub mod model;
pub mod permissions;
pub mod process;
pub mod provider;
pub mod review_cli;
pub mod runtime;
pub mod super_agent;
pub mod sync;
pub mod worker;

use thiserror::Error;

/// Errors at the local runtime boundary. None imply that an uncertain command
/// is safe to execute again.
#[derive(Debug, Error)]
pub enum NodeError {
    /// Planning failed before any execution was authorized.
    #[error("{0}")]
    Planning(#[from] orchestrator_agents::planner::PlannerError),
    /// A proposed goal operation violates collaboration rules.
    #[error("{0}")]
    Collaboration(#[from] orchestrator_domain::collaboration::Rejected),
    /// Local filesystem, lock, process, or stream failure.
    #[error("local I/O: {0}")]
    Io(#[from] std::io::Error),
    /// `SQLite` operation failed.
    #[error("journal: {0}")]
    Database(#[from] sqlx::Error),
    /// Migration failed or its checksum did not match.
    #[error("journal migration: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Invalid serialized local request or stored record.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A local invariant or authorization check rejected the request.
    #[error("{0}")]
    Rejected(&'static str),
    /// Transport or HTTP status failure while synchronizing.
    #[error("coordinator request failed")]
    Transport(#[from] reqwest::Error),
    /// A malformed synchronization report.
    #[error("{0}")]
    Report(#[from] orchestrator_protocol::sync::InvalidReport),
}
#[cfg(test)]
extern crate self as orchestrator_node;
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
pub(crate) mod test_support;

pub mod execution;
pub mod local_provider;

pub mod attachments;

pub mod tool_server;
pub mod tools;
