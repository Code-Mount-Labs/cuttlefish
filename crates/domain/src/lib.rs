//! Pure task lifecycle policy. Persistence and authentication belong to adapters.

use orchestrator_protocol::{task::TaskStatus, values::Epoch};
use thiserror::Error;
pub mod collaboration;

/// Evidence supplied by recovery before replacing an execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousExecution {
    /// The previous executor is confirmed stopped or its effects are reconciled.
    Stopped,
    /// The previous executor or its effects remain unresolved.
    Unknown,
}

/// An authoritative observation of an existing execution during recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionObservation {
    /// The same execution is still running.
    Running,
    /// The same session is awaiting input.
    WaitingInput,
    /// The executor durably reported successful completion.
    Completed,
    /// The executor durably reported a known failure.
    Failed,
    /// The executor confirmed stoppage.
    Cancelled,
}

/// A domain fact or explicit user request, not an arbitrary target state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    /// Execution was authorized, before any work is dispatched.
    Begin,
    /// The executor needs more input.
    RequestInput,
    /// Input was delivered to the existing session.
    Resume,
    /// Work completed, including a race with cancellation.
    Complete,
    /// A known failure ended the attempt.
    Fail,
    /// Contact with a live execution was lost.
    Interrupt,
    /// Request stoppage; this is not evidence it has stopped.
    RequestCancel,
    /// The executor has confirmed stoppage.
    ConfirmCancel,
    /// Reconcile an interrupted task without starting another execution.
    Reconcile {
        /// Epoch attached to the authenticated executor's report.
        execution_epoch: Epoch,
        /// State observed on that executor.
        observed: ExecutionObservation,
    },
    /// Explicitly restart after reconciling the previous execution.
    Restart {
        /// Recovery evidence that prevents blindly retrying unknown effects.
        previous_execution: PreviousExecution,
    },
}

/// A rejected transition leaves the original immutable state untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransitionError {
    /// This action has no meaning in the current state.
    #[error("cannot apply {action:?} to {status:?}")]
    Invalid {
        /// Current state.
        status: TaskStatus,
        /// Rejected action.
        action: TaskAction,
    },
    /// An interrupted execution may still be performing external actions.
    #[error("restart requires a resolved previous execution")]
    UnresolvedExecution,
    /// A report belongs to a different execution attempt.
    #[error("execution epoch mismatch: expected {expected}, received {received}")]
    EpochMismatch {
        /// Current task epoch.
        expected: Epoch,
        /// Epoch carried by the report.
        received: Epoch,
    },
    /// Ownership versions must never wrap and reuse an earlier value.
    #[error("execution epoch exhausted")]
    EpochExhausted,
}

/// Immutable task state: current status plus specialist execution ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState {
    status: TaskStatus,
    execution_epoch: Epoch,
}

impl TaskState {
    /// Create the first queued execution.
    #[must_use]
    pub const fn new() -> Self {
        Self::restore(TaskStatus::Queued, Epoch::INITIAL)
    }

    /// Restore a persisted state. Callers must authenticate its provenance.
    #[must_use]
    pub const fn restore(status: TaskStatus, execution_epoch: Epoch) -> Self {
        Self {
            status,
            execution_epoch,
        }
    }

    /// Current task status.
    #[must_use]
    pub const fn status(self) -> TaskStatus {
        self.status
    }

    /// Specialist ownership version, independent of conversation ownership.
    #[must_use]
    pub const fn execution_epoch(self) -> Epoch {
        self.execution_epoch
    }

    /// Derive a new state without mutating this one.
    ///
    /// # Errors
    /// Rejects invalid transitions, unsafe restarts, and epoch exhaustion.
    pub fn transition(self, action: TaskAction) -> Result<Self, TransitionError> {
        use TaskAction as A;
        use TaskStatus as S;

        let status = match (self.status, action) {
            (S::Queued, A::Begin) | (S::WaitingInput, A::Resume) => S::Running,
            (S::Running, A::RequestInput) => S::WaitingInput,
            (S::Running | S::Cancelling, A::Complete) => S::Completed,
            (S::Queued | S::Running | S::WaitingInput | S::Cancelling, A::Fail) => S::Failed,
            (S::Running | S::WaitingInput | S::Cancelling, A::Interrupt) => S::Interrupted,
            (S::Queued, A::RequestCancel) | (S::Cancelling, A::ConfirmCancel) => S::Cancelled,
            (S::Running | S::WaitingInput | S::Interrupted, A::RequestCancel) => S::Cancelling,
            (
                S::Interrupted,
                A::Reconcile {
                    execution_epoch,
                    observed,
                },
            ) => {
                if execution_epoch != self.execution_epoch {
                    return Err(TransitionError::EpochMismatch {
                        expected: self.execution_epoch,
                        received: execution_epoch,
                    });
                }
                match observed {
                    ExecutionObservation::Running => S::Running,
                    ExecutionObservation::WaitingInput => S::WaitingInput,
                    ExecutionObservation::Completed => S::Completed,
                    ExecutionObservation::Failed => S::Failed,
                    ExecutionObservation::Cancelled => S::Cancelled,
                }
            }
            (S::Failed | S::Interrupted | S::Cancelled, A::Restart { previous_execution }) => {
                if previous_execution == PreviousExecution::Unknown {
                    return Err(TransitionError::UnresolvedExecution);
                }
                let execution_epoch = self
                    .execution_epoch
                    .next()
                    .map_err(|_| TransitionError::EpochExhausted)?;
                return Ok(Self::restore(S::Queued, execution_epoch));
            }
            _ => {
                return Err(TransitionError::Invalid {
                    status: self.status,
                    action,
                });
            }
        };
        Ok(Self::restore(status, self.execution_epoch))
    }
}

impl Default for TaskState {
    fn default() -> Self {
        Self::new()
    }
}
