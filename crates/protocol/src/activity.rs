//! Bounded, factual execution activity. No prompts, file contents or tool arguments.
use crate::{execution::LocalProvider, ids::CommandId};
use serde::{Deserialize, Serialize};

/// Current observable stage of a specialist turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPhase {
    /// Preparing local inputs and checking the workspace.
    Preparing,
    /// Starting the selected provider transport.
    Connecting,
    /// Provider transport accepted the task input.
    Submitted,
    /// Provider reported that it is processing the turn.
    Working,
    /// A provider-authored, user-facing progress message (never private reasoning).
    Commentary,
    /// Provider explicitly invoked a file-reading tool.
    ReadingFiles,
    /// Provider explicitly invoked a file-writing tool.
    WritingFiles,
    /// Provider invoked a command execution tool.
    RunningCommand,
    /// Provider invoked web search or retrieval.
    SearchingWeb,
    /// Provider invoked image generation.
    GeneratingImage,
    /// Provider delegated to another agent.
    Delegating,
    /// Provider invoked another tool whose details remain private.
    UsingTool,
    /// Tool activity ended; waiting for the provider's next response.
    Waiting,
    /// Saving and synchronizing the generated results.
    CollectingResults,
    /// Execution became uncertain and needs operator attention.
    NeedsAttention,
}

/// One immutable journal event. Sequence is scoped to its authenticated node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionActivity {
    /// Positive, durable node-local cursor.
    pub sequence: i64,
    /// Exact dispatch that produced this observation.
    pub command_id: CommandId,
    /// Provider actually selected by the execution node.
    pub provider: LocalProvider,
    /// Observed stage, never an inferred percentage.
    pub phase: ActivityPhase,
    /// Node-observed Unix time in milliseconds.
    pub at_ms: i64,
    /// Optional public progress text, bounded to 2000 UTF-8 bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
