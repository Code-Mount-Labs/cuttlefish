//! Configured local specialist providers and safe workspace advertisements.
use crate::ids::WorkspaceId;
use serde::{Deserialize, Serialize};

/// A supported, locally authenticated provider application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalProvider {
    /// Codex app-server using its existing local sign-in.
    Codex,
    /// Claude Code using its existing local sign-in.
    Claude,
}
impl LocalProvider {
    /// Executable name and advertised capability.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}
/// Public workspace metadata; credentials and local paths never leave the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderWorkspace {
    /// Stable logical workspace.
    pub workspace_id: WorkspaceId,
    /// Human-readable label.
    pub name: String,
    /// Configured local specialist.
    pub provider: LocalProvider,
    /// Operator explicitly permits file edits within this workspace.
    pub allow_edits: bool,
}
