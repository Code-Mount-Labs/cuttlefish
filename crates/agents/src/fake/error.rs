use orchestrator_protocol::values::Epoch;
use thiserror::Error;

/// Expected failures from the fake; never encoded as a successful reply.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FakeAgentError {
    /// The command is addressed to a different group or computer.
    #[error("command is outside this agent's group or node")]
    WrongRecipient,
    /// An existing command ID was reused with a different payload.
    #[error("command ID was reused with different content")]
    CommandConflict,
    /// Session creation cannot silently replace an existing session.
    #[error("session already exists")]
    SessionExists,
    /// A follow-up requires an existing session.
    #[error("session does not exist")]
    SessionNotFound,
    /// A session belongs to a different logical task.
    #[error("session belongs to a different task")]
    WrongTask,
    /// A specialist execution cannot be changed by an ordinary message.
    #[error("execution epoch mismatch: expected {expected}, received {received}")]
    EpochMismatch {
        /// Session's current authority.
        expected: Epoch,
        /// Command's supplied authority.
        received: Epoch,
    },
    /// Cancelled sessions cannot accept further messages.
    #[error("session is cancelled")]
    SessionCancelled,
    /// A message was already applied through a different command ID.
    #[error("message already exists in this session")]
    DuplicateMessage,
    /// Deterministic test failure, injected before any command effects.
    #[error("injected failure before execution")]
    InjectedFailure,
}
