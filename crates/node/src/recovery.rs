//! Local checkpoint inspection and explicitly authorized continuation.
use crate::{
    NodeError,
    journal::Journal,
    model::{CommandStatus, LaunchSpec, SessionStatus},
    process::Launcher,
    runtime::NodeRuntime,
};
use orchestrator_protocol::{
    agent::AgentCommand,
    recovery::{Checkpoint, RecoveryJob, RecoveryState, RecoveryView},
};
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use std::path::Path;
use uuid::Uuid;

impl Journal {
    pub(crate) async fn provider_quiescence(
        &self,
        command: &AgentCommand,
        instance: &str,
        stopped: bool,
    ) -> Result<(), NodeError> {
        let result=sqlx::query("INSERT INTO provider_quiescence(command_id,stopped) SELECT ?,? WHERE EXISTS(SELECT 1 FROM command_receipts c JOIN local_sessions s USING(session_id) WHERE c.command_id=? AND c.status='executing' AND s.instance_id=?) ON CONFLICT(command_id) DO UPDATE SET stopped=excluded.stopped")
            .bind(command.command_id.to_string()).bind(stopped).bind(command.command_id.to_string()).bind(instance).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(NodeError::Rejected("stale provider execution"));
        }
        Ok(())
    }
    async fn checkpoint(&self, command: &AgentCommand) -> Result<Checkpoint, NodeError> {
        if command.group_id != self.identity().group_id
            || command.node_id != self.identity().node_id
        {
            return Err(NodeError::Rejected("Recovery belongs to another agent."));
        }
        let receipt = self
            .command(command.command_id)
            .await?
            .ok_or(NodeError::Rejected("Saved task is unavailable."))?;
        if receipt.request.0 != *command || receipt.status != CommandStatus::Uncertain {
            return Err(NodeError::Rejected("The task is no longer interrupted."));
        }
        let session = self
            .session(command.target.session_id)
            .await?
            .ok_or(NodeError::Rejected("Saved session is unavailable."))?;
        if session.state != SessionStatus::Interrupted || session.stop_requested {
            return Err(NodeError::Rejected(
                "The previous execution has not stopped safely.",
            ));
        }
        let safe:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_quiescence WHERE command_id=? AND stopped=1) AND NOT EXISTS(SELECT 1 FROM runtime_permissions WHERE command_id=? AND json_extract(record,'$.state') NOT IN ('delivered','abandoned')) AND NOT EXISTS(SELECT 1 FROM provider_tools WHERE parent=? AND response IS NULL) AND NOT EXISTS(SELECT 1 FROM command_receipts WHERE session_id=? AND command_id<>? AND status='uncertain')")
            .bind(command.command_id.to_string()).bind(command.command_id.to_string()).bind(command.command_id.to_string()).bind(command.target.session_id.to_string()).bind(command.command_id.to_string()).fetch_one(&self.pool).await?;
        if !safe {
            return Err(NodeError::Rejected(
                "Recovery cannot confirm the old provider stopped, or another command, approval or tool result is still unresolved. Older tasks may lack the required stop record. No work was restarted.",
            ));
        }
        let binding = self
            .provider_binding(command.target.session_id)
            .await?
            .filter(|b| b.resumable && b.target == command.target)
            .ok_or(NodeError::Rejected(
                "The original provider session cannot be resumed. Saved files remain intact.",
            ))?;
        let config = self
            .execution_config(command.target.session_id)
            .await?
            .ok_or(NodeError::Rejected(
                "The original workspace configuration is unavailable.",
            ))?;
        if binding.provider
            != orchestrator_protocol::capabilities::Capability::local_provider(
                config.workspace.provider,
            )
        {
            return Err(NodeError::Rejected(
                "The workspace provider changed. Restore the original provider before continuing.",
            ));
        }
        crate::execution::check_directory(&config, &config.directory)?;
        let mut digest = Sha256::new();
        digest.update(serde_json::json!([command, binding, &config]).to_string());
        let files = inspect_files(
            &config.directory,
            &config.directory,
            0,
            &mut digest,
            &mut ScanBudget::default(),
        )?;
        let progress:Option<String>=sqlx::query_scalar("SELECT summary FROM execution_activity WHERE command_id=? AND summary IS NOT NULL AND phase<>'\"needs_attention\"' ORDER BY sequence DESC LIMIT 1")
            .bind(command.command_id.to_string()).fetch_optional(&self.pool).await?.flatten();
        Ok(Checkpoint {
            provider: config.workspace.provider,
            session_id: command.target.session_id,
            files,
            progress,
            digest: format!("{:x}", digest.finalize()),
        })
    }
    async fn inspect_recovery(&self, job: &RecoveryJob) -> Result<RecoveryView, NodeError> {
        let mut view = job.view.clone();
        match self.checkpoint(&job.command).await {
            Ok(checkpoint) => {
                view.state = RecoveryState::Ready;
                view.checkpoint = Some(checkpoint);
                view.detail = None;
            }
            Err(error) => {
                view.state = RecoveryState::Blocked;
                view.checkpoint = None;
                view.detail = Some(explanation(&error).into());
            }
        }
        sqlx::query("INSERT INTO task_recovery(request_id,command_id,view) VALUES(?,?,?)")
            .bind(view.id.to_string())
            .bind(view.command_id.to_string())
            .bind(Json(&view))
            .execute(&self.pool)
            .await?;
        Ok(view)
    }
    async fn recovery_plan(
        &self,
        job: &RecoveryJob,
    ) -> Result<(RecoveryView, Option<LaunchSpec>), NodeError> {
        if job.view.command_id != job.command.command_id
            || job.command.group_id != self.identity().group_id
            || job.command.node_id != self.identity().node_id
        {
            return Err(NodeError::Rejected("Invalid recovery destination."));
        }
        let saved: Option<(Json<RecoveryView>, Option<String>)> = sqlx::query_as(
            "SELECT view,instance FROM task_recovery WHERE request_id=? AND command_id=?",
        )
        .bind(job.view.id.to_string())
        .bind(job.view.command_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        let Some((Json(mut view), instance)) = saved else {
            if job.view.state != RecoveryState::Inspecting {
                return Err(NodeError::Rejected("Recovery inspection is missing."));
            }
            return Ok((self.inspect_recovery(job).await?, None));
        };
        if job.view.state == RecoveryState::Inspecting || view.state == RecoveryState::Blocked {
            return Ok((view, None));
        }
        if job.view.state != RecoveryState::Approved || job.view.checkpoint != view.checkpoint {
            return Err(NodeError::Rejected(
                "Recovery approval does not match its checkpoint.",
            ));
        }
        if view.state == RecoveryState::Resumed {
            let launch = self.retry_launch(&job.command, instance.as_deref()).await?;
            return Ok((view, launch));
        }
        if view.state != RecoveryState::Ready || view.checkpoint.is_none() {
            return Err(NodeError::Rejected("Recovery has no reviewed checkpoint."));
        }
        let current = self.checkpoint(&job.command).await;
        if current.as_ref().ok() != view.checkpoint.as_ref() {
            view.state = RecoveryState::Blocked;
            view.detail = Some(
                "Saved state changed after inspection. Inspect again before continuing.".into(),
            );
            sqlx::query("UPDATE task_recovery SET view=? WHERE request_id=?")
                .bind(Json(&view))
                .bind(view.id.to_string())
                .execute(&self.pool)
                .await?;
            return Ok((view, None));
        }
        let instance = Uuid::new_v4().to_string();
        let launch = self.recovery_launch(&job.command, instance.clone()).await?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed=sqlx::query("UPDATE local_sessions SET instance_id=?,state='starting',pid=NULL,last_error=NULL,heartbeat_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE session_id=? AND state='interrupted' AND stop_requested=0 AND EXISTS(SELECT 1 FROM command_receipts WHERE command_id=? AND status='uncertain') AND EXISTS(SELECT 1 FROM provider_quiescence WHERE command_id=? AND stopped=1)")
            .bind(&instance).bind(job.command.target.session_id.to_string()).bind(view.command_id.to_string()).bind(view.command_id.to_string()).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(NodeError::Rejected(
                "Recovery state changed before continuation.",
            ));
        }
        sqlx::query(
            "UPDATE command_receipts SET status='queued' WHERE command_id=? AND status='uncertain'",
        )
        .bind(view.command_id.to_string())
        .execute(&mut *tx)
        .await?;
        view.state = RecoveryState::Resumed;
        sqlx::query(
            "UPDATE task_recovery SET view=?,instance=? WHERE request_id=? AND instance IS NULL",
        )
        .bind(Json(&view))
        .bind(instance)
        .bind(view.id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((view, Some(launch)))
    }
    async fn retry_launch(
        &self,
        command: &AgentCommand,
        instance: Option<&str>,
    ) -> Result<Option<LaunchSpec>, NodeError> {
        let session = self
            .session(command.target.session_id)
            .await?
            .ok_or(NodeError::Rejected("Recovery session disappeared."))?;
        if Some(session.instance_id.as_str()) != instance {
            return Ok(None);
        }
        if session.state == SessionStatus::Interrupted && session.pid.is_none() {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            let changed=sqlx::query("UPDATE local_sessions SET state='starting',heartbeat_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE session_id=? AND instance_id=? AND state='interrupted' AND pid IS NULL AND stop_requested=0")
                .bind(command.target.session_id.to_string()).bind(&session.instance_id).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(NodeError::Rejected("Recovery launch was stopped."));
            }
            sqlx::query("UPDATE command_receipts SET status='queued' WHERE command_id=? AND status='uncertain'").bind(command.command_id.to_string()).execute(&mut *tx).await?;
            tx.commit().await?;
        } else if session.state != SessionStatus::Starting {
            return Ok(None);
        }
        Ok(Some(
            self.recovery_launch(command, session.instance_id).await?,
        ))
    }
    async fn recovery_launch(
        &self,
        command: &AgentCommand,
        instance_id: String,
    ) -> Result<LaunchSpec, NodeError> {
        let config = self
            .execution_config(command.target.session_id)
            .await?
            .ok_or(NodeError::Rejected("Recovery workspace disappeared."))?;
        Ok(LaunchSpec {
            command: command.clone(),
            instance_id,
            directory: config.directory,
        })
    }
    pub(crate) async fn recovery_message(
        &self,
        command: &AgentCommand,
        original: &str,
        instructions: &Path,
    ) -> Result<Option<String>, NodeError> {
        let view:Option<Json<RecoveryView>>=sqlx::query_scalar("SELECT view FROM task_recovery WHERE command_id=? AND instance IS NOT NULL ORDER BY rowid DESC LIMIT 1")
            .bind(command.command_id.to_string()).fetch_optional(&self.pool).await?;
        let Some(view) = view else { return Ok(None) };
        let binding = self
            .provider_binding(command.target.session_id)
            .await?
            .filter(|b| b.resumable)
            .ok_or(NodeError::Rejected(
                "Recovery requires the original provider session; starting over is refused.",
            ))?;
        if binding.target != command.target {
            return Err(NodeError::Rejected("Recovery provider session changed."));
        }
        let root = instructions.parent().and_then(Path::parent);
        let prompt = match root.filter(|p| p.join("execution/recovery.md").exists()) {
            Some(root) => {
                orchestrator_prompts::Prompt::load(root, orchestrator_prompts::Kind::Recovery)?
            }
            None => orchestrator_prompts::Prompt::bundled(orchestrator_prompts::Kind::Recovery),
        };
        Ok(Some(format!(
            "{}\n{}",
            prompt.as_str(),
            serde_json::json!({"original_request":original,"checkpoint":view.checkpoint})
        )))
    }
}
fn explanation(error: &NodeError) -> &str {
    match error {
        NodeError::Rejected(reason) => reason,
        _ => {
            "Saved state could not be inspected. Restore access to the original workspace and try again."
        }
    }
}

/// Bound both filesystem traversal and bytes read for a user-requested inspection.
struct ScanBudget {
    entries: u32,
    bytes: u64,
}
impl Default for ScanBudget {
    fn default() -> Self {
        Self {
            entries: 0,
            bytes: 256 * 1024 * 1024,
        }
    }
}
fn inspect_files(
    root: &Path,
    path: &Path,
    depth: usize,
    digest: &mut Sha256,
    budget: &mut ScanBudget,
) -> Result<u32, NodeError> {
    if depth > 16 {
        return Err(NodeError::Rejected(
            "Workspace inspection exceeds the directory depth limit.",
        ));
    }
    if !path.canonicalize()?.starts_with(root) {
        return Err(NodeError::Rejected(
            "Workspace directory changed during inspection.",
        ));
    }
    let mut paths = std::fs::read_dir(path)?
        .take(10001)
        .map(|entry| entry.map(|entry| entry.path()));
    inspect_entries(root, &mut paths, depth, digest, budget)
}
fn inspect_entries(
    root: &Path,
    paths: &mut dyn Iterator<Item = std::io::Result<std::path::PathBuf>>,
    depth: usize,
    digest: &mut Sha256,
    budget: &mut ScanBudget,
) -> Result<u32, NodeError> {
    let mut paths = paths.collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut files = 0;
    for path in paths {
        budget.entries += 1;
        if budget.entries > 10000 {
            return Err(NodeError::Rejected(
                "Workspace inspection exceeds 10000 entries.",
            ));
        }
        if path.file_name().is_some_and(|n| {
            [".git", "node_modules", "target", ".venv"]
                .iter()
                .any(|s| n == *s)
        }) {
            continue;
        }
        let metadata = path.symlink_metadata()?;
        digest.update(
            path.strip_prefix(root)
                .map_err(|_| NodeError::Rejected("Workspace inspection escaped its root."))?
                .as_os_str()
                .as_encoded_bytes(),
        );
        digest.update(metadata.len().to_le_bytes());
        if metadata.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            files += inspect_files(root, &path, depth + 1, digest, budget)?;
        } else if metadata.is_file() {
            hash_file(&path, digest, &mut budget.bytes)?;
            files += 1;
        }
    }
    Ok(files)
}
fn hash_file(path: &Path, digest: &mut Sha256, budget: &mut u64) -> Result<(), NodeError> {
    hash_contents(std::fs::File::open(path)?, digest, budget)
}
fn hash_contents(
    mut input: impl std::io::Read,
    digest: &mut Sha256,
    budget: &mut u64,
) -> Result<(), NodeError> {
    let mut buffer = [0; 8192];
    loop {
        let length = input.read(&mut buffer)?;
        if length == 0 {
            return Ok(());
        }
        *budget=budget.checked_sub(length as u64).ok_or(NodeError::Rejected("Workspace inspection exceeds 256 MiB. Move generated files outside this workspace before retrying."))?;
        digest.update(&buffer[..length]);
    }
}
impl<L: Launcher> NodeRuntime<L> {
    pub(crate) async fn recover(&mut self, job: &RecoveryJob) -> Result<RecoveryView, NodeError> {
        let (view, launch) = self.journal().recovery_plan(job).await?;
        if let Some(spec) = launch {
            self.launch_recovery(spec)?;
        }
        Ok(view)
    }
}

#[cfg(test)]
#[path = "../tests/internal/recovery.rs"]
mod tests;
