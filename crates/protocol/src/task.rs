//! Stable task states exposed over the wire; transition policy lives in domain.

use serde::{Deserialize, Serialize};

/// Lifecycle of one logical task, including its current execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Accepted; execution has not yet been authorized or dispatched.
    Queued,
    /// An executor is performing work.
    Running,
    /// The existing session is waiting for another message.
    WaitingInput,
    /// Cancellation was requested but not yet confirmed.
    Cancelling,
    /// The current execution completed successfully.
    Completed,
    /// The current execution failed with a known outcome.
    Failed,
    /// Execution was lost; its external outcome may be unknown.
    Interrupted,
    /// Stoppage or cancellation before execution was confirmed.
    Cancelled,
}
