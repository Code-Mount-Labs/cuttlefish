//! Local installation management. Configuration and database credentials remain
//! on the user's machine; the dashboard starts before `PostgreSQL` is configured.

pub mod auth;
pub mod chat;
pub mod cli;
pub mod connector;
pub mod dependencies;
pub mod docker;
mod files;
mod passwords;
pub mod registry;
pub mod remote;
pub mod resources;
pub mod runtime;
pub mod settings;
pub mod state;
pub mod web;

use thiserror::Error;

/// Redacted local-management errors safe to return from its JSON API.
#[derive(Debug, Error)]
pub enum Error {
    /// Rejected input or state transition.
    #[error("{0}")]
    Invalid(&'static str),
    /// Local filesystem operation failed.
    #[error("local configuration file unavailable")]
    Io(#[from] std::io::Error),
    /// Local journal or user-owned database failed.
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    /// Local schema migration failed.
    #[error("local configuration migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Stored configuration failed to decode.
    #[error("invalid stored configuration")]
    Json(#[from] serde_json::Error),
    /// Administrator authentication failed.
    #[error("authentication required")]
    Unauthorized,
    /// Password worker admission or attempt limit reached.
    #[error("too many attempts; try again later")]
    Limited,
    /// Password worker failed.
    #[error("password service unavailable")]
    Worker(#[from] tokio::task::JoinError),
    /// A dependency did not complete the requested operation.
    #[error("{0}")]
    Dependency(&'static str),
}

#[cfg(test)]
#[path = "../tests/internal/mod.rs"]
mod tests;
