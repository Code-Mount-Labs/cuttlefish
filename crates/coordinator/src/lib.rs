//! Shared routing records and authenticated synchronization. No task execution.

pub mod catalog_source;
pub mod collaboration;
mod collaboration_web;
mod dispatch;
pub mod routing;
pub mod selector;

#[cfg(test)]
extern crate self as orchestrator_coordinator;
pub mod store;
pub mod web;

#[cfg(test)]
#[path = "../tests/internal/suites.rs"]
mod tests;

use thiserror::Error;

/// Failures at the coordination boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// Goal lifecycle, plan, approval or authorization conflict.
    #[error("{0}")]
    Collaboration(#[from] orchestrator_domain::collaboration::Rejected),
    /// Storage is unavailable or violated a constraint.
    #[error("coordination storage: {0}")]
    Database(#[from] sqlx::Error),
    /// Schema cannot be installed or has an unexpected checksum.
    #[error("coordination migration: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Credential is absent, unknown, or revoked.
    #[error("node authentication required")]
    Unauthorized,
    /// An immutable event identity was reused.
    #[error("event identity conflicts with its saved payload")]
    Conflict,
    /// A report violates the shared contract.
    #[error("{0}")]
    Invalid(#[from] orchestrator_protocol::sync::InvalidReport),
    /// Request conflicts with a conversation or outstanding command.
    #[error("{0}")]
    RoutingConflict(&'static str),
    /// Execution may have happened; automatic replay is forbidden.
    #[error("execution is unresolved; retain affinity or confirm stoppage before restart")]
    Unresolved,
    /// No fresh, mounted, capable node has capacity.
    #[error("no eligible execution node is available")]
    NoCapacity,
    /// Model request failed or returned an invalid selection.
    #[error("routing model unavailable or returned an invalid choice")]
    Model,
}

pub mod chat;

pub mod attachments;
