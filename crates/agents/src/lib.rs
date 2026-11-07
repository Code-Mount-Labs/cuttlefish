//! Specialist adapter boundary and a deterministic in-memory test implementation.
//!
//! The fake is not a durable runtime: receipts survive only while it lives.
//! The node crate supplies durable receipts and process supervision around adapters.

pub mod fake;
pub mod planner;
pub mod provider;

use std::{error::Error, future::Future};

use orchestrator_protocol::agent::{AgentCommand, AgentEvent};

/// A small statically dispatched boundary for asynchronous specialist adapters.
///
/// Implementations must return explicit failures and preserve session identity.
/// Persistence, retries, and admission control are runtime responsibilities.
pub trait Agent: Send {
    /// Adapter-specific failures without coupling other adapters to this one.
    type Error: Error + Send + Sync;

    /// Whether an adapter failure may have followed an external effect.
    /// Such failures must be fenced as uncertain rather than safely rejected.
    fn failure_is_uncertain(&self) -> bool {
        false
    }

    /// Execute a typed command.
    ///
    /// # Errors
    /// Returns the adapter's error when execution is rejected or fails.
    fn execute(
        &mut self,
        command: &AgentCommand,
    ) -> impl Future<Output = Result<AgentEvent, Self::Error>> + Send;
}
