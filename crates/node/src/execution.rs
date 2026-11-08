//! Local provider configuration and durable specialist selection.
use crate::{NodeError, journal::Journal};
use orchestrator_protocol::{execution::ProviderWorkspace, ids::SessionId};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use std::path::{Path, PathBuf};

/// Immutable operator-approved executable and workspace for a specialist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Metadata safe to advertise to the router.
    pub workspace: ProviderWorkspace,
    /// Absolute, operator-selected provider executable.
    pub executable: PathBuf,
    /// Canonical local workspace directory.
    pub directory: PathBuf,
    /// Reviewed instruction file, kept separate from Rust source.
    pub instructions: PathBuf,
}
impl Journal {
    /// Register a provider workspace without changing an existing configuration.
    ///
    /// # Errors
    /// Rejects invalid paths, changed configuration and unavailable storage.
    pub async fn configure_execution(&self, config: &ExecutionConfig) -> Result<(), NodeError> {
        let path = config
            .directory
            .to_str()
            .ok_or(NodeError::Rejected("workspace must be UTF-8"))?;
        if config.workspace.name.trim().is_empty()
            || config.workspace.name.len() > 128
            || !config.executable.is_absolute()
            || !config.executable.is_file()
            || !config.instructions.is_file()
            || config.directory.canonicalize()? != config.directory
            || !config.directory.is_dir()
        {
            return Err(NodeError::Rejected(
                "invalid provider workspace configuration",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let id = config.workspace.workspace_id.to_string();
        let existing: Option<Json<ExecutionConfig>> = sqlx::query_scalar(
            "SELECT configuration FROM workspace_providers WHERE workspace_id=?",
        )
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;
        if existing.is_none() {
            let used: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM local_sessions WHERE workspace_id=?)",
            )
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
            if used {
                return Err(NodeError::Rejected(
                    "cannot replace an existing session's adapter",
                ));
            }
        }
        if existing.is_some_and(|saved| saved.0 != *config) {
            return Err(NodeError::Rejected(
                "provider workspace configuration is immutable",
            ));
        }
        let mounted: Option<String> = sqlx::query_scalar("INSERT INTO workspace_mounts(workspace_id,path) VALUES (?,?) ON CONFLICT(workspace_id) DO UPDATE SET path=workspace_mounts.path WHERE workspace_mounts.path=excluded.path RETURNING path")
            .bind(&id).bind(path).fetch_optional(&mut *tx).await?;
        if mounted.is_none() {
            return Err(NodeError::Rejected("workspace mount conflict"));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO workspace_providers(workspace_id,configuration) VALUES (?,?)",
        )
        .bind(id)
        .bind(Json(config))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    /// List registered local providers without exposing credentials.
    ///
    /// # Errors
    /// Reports unavailable or corrupt storage.
    pub async fn executions(&self) -> Result<Vec<ExecutionConfig>, NodeError> {
        let rows: Vec<Json<ExecutionConfig>> = sqlx::query_scalar(
            "SELECT configuration FROM workspace_providers ORDER BY workspace_id LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }
    /// Resolve a session's immutable workspace configuration.
    ///
    /// # Errors
    /// Reports storage errors. Unconfigured workspaces retain the fake adapter.
    pub async fn execution_config(
        &self,
        session: SessionId,
    ) -> Result<Option<ExecutionConfig>, NodeError> {
        let row: Option<Json<ExecutionConfig>> = sqlx::query_scalar("SELECT configuration FROM workspace_providers JOIN local_sessions USING(workspace_id) WHERE session_id=?")
            .bind(session.to_string()).fetch_optional(&self.pool).await?;
        Ok(row.map(|row| row.0))
    }
}
/// Resolve a known provider name from an operator-controlled search path.
#[must_use]
pub fn executable(name: &str, search: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(search)
        .map(|root| root.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
}
/// Validate that an immutable configuration still refers to its original directory.
///
/// # Errors
/// Rejects replaced paths or missing directories before spawning any provider.
pub fn check_directory(config: &ExecutionConfig, cwd: &Path) -> Result<(), NodeError> {
    if config.directory.canonicalize()? != config.directory
        || cwd.canonicalize()? != config.directory
    {
        return Err(NodeError::Rejected("provider workspace changed"));
    }
    Ok(())
}

impl crate::sync::Remote {
    /// Advertise provider workspaces through the authenticated router connection.
    ///
    /// # Errors
    /// Reports local storage and authenticated transport failures.
    pub async fn advertise_providers(&self, journal: &Journal) -> Result<(), NodeError> {
        let items: Vec<_> = journal
            .executions()
            .await?
            .into_iter()
            .map(|config| config.workspace)
            .collect();
        self.exchange(
            "/v1/workspaces",
            reqwest::Method::POST,
            Some(serde_json::json!(items)),
        )
        .await?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "../tests/internal/execution.rs"]
mod tests;
