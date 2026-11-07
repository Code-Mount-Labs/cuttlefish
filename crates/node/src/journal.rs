//! Crash-consistent command receipts, process identities, and result outbox.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEventKind},
    ids::{CommandId, SessionId, WorkspaceId},
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    types::Json,
};
use uuid::Uuid;

use crate::{
    NodeError,
    model::{
        Acceptance, CommandOutcome, CommandRecord, CommandStatus, LaunchSpec, NodeIdentity,
        PendingEvent, SessionRecord, SessionStatus,
    },
};

const RECEIPT: &str =
    "SELECT sequence, request, status, outcome FROM command_receipts WHERE command_id = ?";
const SESSION: &str = "SELECT target, workspace_id, instance_id, state, pid, heartbeat_at, stop_requested, last_error FROM local_sessions WHERE session_id = ?";

/// A node-local database. Clones share a single-connection pool; other processes
/// have their own connections to the same WAL database on this computer.
#[derive(Debug, Clone)]
pub struct Journal {
    pub(crate) pool: SqlitePool,
    path: PathBuf,
    identity: NodeIdentity,
}

impl Journal {
    /// Create or migrate a journal, binding it permanently to one node identity.
    ///
    /// # Errors
    /// Reports storage errors and rejects a different identity on an existing DB.
    pub async fn open(path: &Path, identity: NodeIdentity) -> Result<Self, NodeError> {
        let pool = connect(path, true).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("INSERT OR IGNORE INTO node_identity(singleton, identity) VALUES (1, ?)")
            .bind(Json(identity))
            .execute(&pool)
            .await?;
        if read_identity(&pool).await? != identity {
            return Err(NodeError::Rejected(
                "journal belongs to a different node or group",
            ));
        }
        Ok(Self {
            pool,
            path: path.to_path_buf(),
            identity,
        })
    }

    /// Attach a specialist to an existing journal. Never creates an empty DB.
    ///
    /// # Errors
    /// Reports missing, invalid, or unavailable storage.
    pub async fn attach(path: &Path) -> Result<Self, NodeError> {
        let pool = connect(path, false).await?;
        let identity = read_identity(&pool).await?;
        Ok(Self {
            pool,
            path: path.to_path_buf(),
            identity,
        })
    }

    /// Local journal filename.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Identity established when this database was created.
    #[must_use]
    pub const fn identity(&self) -> NodeIdentity {
        self.identity
    }

    /// Close this process's connections, useful for orderly shutdown.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Register a canonical local directory. An active session prevents retargeting.
    ///
    /// # Errors
    /// Rejects missing directories, non-UTF-8 paths, and active mount changes.
    pub async fn mount(&self, workspace_id: WorkspaceId, path: &Path) -> Result<(), NodeError> {
        let path = canonical_workspace(path)?;
        let result = sqlx::query("INSERT INTO workspace_mounts(workspace_id,path) VALUES (?,?) ON CONFLICT(workspace_id) DO UPDATE SET path=excluded.path WHERE workspace_mounts.path=excluded.path OR NOT EXISTS (SELECT 1 FROM local_sessions WHERE workspace_id=excluded.workspace_id AND state <> 'stopped')")
            .bind(workspace_id.to_string()).bind(path).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(NodeError::Rejected(
                "workspace has an active or unresolved session",
            ));
        }
        Ok(())
    }

    /// Commit a command before acknowledging it. Only a new start produces a
    /// launch permit; retries and recovery never produce another permit.
    ///
    /// # Errors
    /// Rejects identity, payload, session, workspace, and epoch mismatches.
    pub async fn accept(
        &self,
        command: &AgentCommand,
        instance: Uuid,
    ) -> Result<Acceptance, NodeError> {
        if command.group_id != self.identity.group_id || command.node_id != self.identity.node_id {
            return Err(NodeError::Rejected(
                "command is addressed to a different node or group",
            ));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(record) = sqlx::query_as::<_, CommandRecord>(RECEIPT)
            .bind(command.command_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
        {
            if record.request.0 != *command {
                return Err(NodeError::Rejected(
                    "command ID conflicts with its saved payload",
                ));
            }
            // Read-only replay: dropping the transaction rolls it back. There
            // are no new effects to commit or additional launch rights to grant.
            return Ok(Acceptance {
                record,
                launch: None,
            });
        }
        let existing = sqlx::query_as::<_, SessionRecord>(SESSION)
            .bind(command.target.session_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        let launch = match &command.action {
            AgentAction::Start { workspace_id, .. } => {
                if existing.is_some() {
                    return Err(NodeError::Rejected(
                        "session already exists; recovery cannot replace it",
                    ));
                }
                if instance.is_nil() {
                    return Err(NodeError::Rejected("launch identity cannot be nil"));
                }
                let directory: String =
                    sqlx::query_scalar("SELECT path FROM workspace_mounts WHERE workspace_id=?")
                        .bind(workspace_id.to_string())
                        .fetch_optional(&mut *tx)
                        .await?
                        .ok_or(NodeError::Rejected("workspace is not mounted"))?;
                sqlx::query("INSERT INTO local_sessions(session_id,target,workspace_id,instance_id,state,heartbeat_at) VALUES (?,?,?,?,'starting',CAST(unixepoch('subsec')*1000 AS INTEGER))")
                    .bind(command.target.session_id.to_string()).bind(Json(command.target))
                    .bind(workspace_id.to_string()).bind(instance.to_string()).execute(&mut *tx).await?;
                Some(LaunchSpec {
                    command: command.clone(),
                    instance_id: instance.to_string(),
                    directory: PathBuf::from(directory),
                })
            }
            AgentAction::Send { .. } | AgentAction::Cancel => {
                let session = existing.ok_or(NodeError::Rejected("session does not exist"))?;
                if session.target.0 != command.target {
                    return Err(NodeError::Rejected(
                        "session task or execution epoch mismatch",
                    ));
                }
                if !matches!(
                    session.state,
                    SessionStatus::Starting | SessionStatus::Running
                ) || session.stop_requested
                {
                    return Err(NodeError::Rejected("session is stopped or unresolved"));
                }
                None
            }
        };
        let record = sqlx::query_as::<_, CommandRecord>("INSERT INTO command_receipts(command_id,session_id,request,status) VALUES (?,?,?,'queued') RETURNING sequence, request, status, outcome")
            .bind(command.command_id.to_string()).bind(command.target.session_id.to_string())
            .bind(Json(command)).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Acceptance { record, launch })
    }

    /// Read a durable receipt; this never dispatches or replays its command.
    ///
    /// # Errors
    /// Reports journal errors, including corrupt serialized records.
    pub async fn command(&self, id: CommandId) -> Result<Option<CommandRecord>, NodeError> {
        Ok(sqlx::query_as(RECEIPT)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Read a session's last confirmed process identity and state.
    ///
    /// # Errors
    /// Reports unavailable or corrupt journal records.
    pub async fn session(&self, id: SessionId) -> Result<Option<SessionRecord>, NodeError> {
        Ok(sqlx::query_as(SESSION)
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Claim the one-time worker registration. An old or second process fails.
    ///
    /// # Errors
    /// Rejects stale launch identities, repeat registrations, or invalid PIDs.
    pub async fn register(&self, id: SessionId, instance: &str, pid: u32) -> Result<(), NodeError> {
        let result = sqlx::query("UPDATE local_sessions SET state='running',pid=?,heartbeat_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE session_id=? AND instance_id=? AND state='starting' AND stop_requested=0")
            .bind(i64::from(pid)).bind(id.to_string()).bind(instance).execute(&self.pool).await?;
        ensure_changed(result.rows_affected())
    }

    /// Refresh liveness for the original worker. A stopped session cannot revive.
    ///
    /// # Errors
    /// Rejects a stale worker or unavailable journal.
    pub async fn heartbeat(&self, id: SessionId, instance: &str) -> Result<(), NodeError> {
        let result = sqlx::query("UPDATE local_sessions SET state='running',heartbeat_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE session_id=? AND instance_id=? AND state IN ('running','interrupted')")
            .bind(id.to_string()).bind(instance).execute(&self.pool).await?;
        ensure_changed(result.rows_affected())
    }

    /// Claim the next FIFO command once, committing execution intent first.
    ///
    /// # Errors
    /// Reports journal errors. Uncertain commands are deliberately not selected.
    pub async fn claim(
        &self,
        id: SessionId,
        instance: &str,
    ) -> Result<Option<CommandRecord>, NodeError> {
        Ok(sqlx::query_as("UPDATE command_receipts SET status='executing' WHERE sequence=(SELECT sequence FROM command_receipts WHERE session_id=? AND status='queued' AND EXISTS (SELECT 1 FROM local_sessions WHERE session_id=? AND instance_id=? AND state='running' AND stop_requested=0) AND NOT EXISTS (SELECT 1 FROM command_receipts WHERE session_id=? AND status IN ('executing','uncertain')) ORDER BY sequence LIMIT 1) RETURNING sequence,request,status,outcome")
            .bind(id.to_string()).bind(id.to_string()).bind(instance).bind(id.to_string()).fetch_optional(&self.pool).await?)
    }

    /// Atomically save a result and its outgoing event. Completion is idempotent.
    ///
    /// # Errors
    /// Rejects unclaimed commands, changed outcomes, stale workers, and misrouted events.
    pub async fn complete(
        &self,
        id: CommandId,
        instance: &str,
        outcome: &CommandOutcome,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let record: CommandRecord = sqlx::query_as(RECEIPT)
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        validate_outcome(&record.request, outcome)?;
        let session: SessionRecord = sqlx::query_as(SESSION)
            .bind(record.request.target.session_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if session.instance_id != instance {
            return Err(NodeError::Rejected("worker no longer owns this session"));
        }
        if let Some(saved) = record.outcome {
            if &saved.0 != outcome {
                return Err(NodeError::Rejected(
                    "completed command outcome cannot change",
                ));
            }
            // Idempotent, read-only result lookup; there is nothing new to commit.
            return Ok(());
        }
        if !matches!(
            record.status,
            CommandStatus::Executing | CommandStatus::Uncertain
        ) {
            return Err(NodeError::Rejected("command was not claimed"));
        }
        let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_permissions WHERE command_id=? AND json_extract(record,'$.state') NOT IN ('delivered','abandoned'))")
            .bind(id.to_string()).fetch_one(&mut *tx).await?;
        if blocked {
            return Err(NodeError::Rejected(
                "command has an unresolved runtime permission",
            ));
        }
        let status = match outcome {
            CommandOutcome::Completed { .. } => CommandStatus::Completed,
            CommandOutcome::Rejected { .. } => CommandStatus::Rejected,
        };
        sqlx::query("UPDATE command_receipts SET status=?,outcome=? WHERE command_id=?")
            .bind(status)
            .bind(Json(outcome))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO pending_events(command_id,outcome) VALUES (?,?)")
            .bind(id.to_string())
            .bind(Json(outcome))
            .execute(&mut *tx)
            .await?;
        if matches!(outcome, CommandOutcome::Completed { event } if event.kind == AgentEventKind::Cancelled)
        {
            sqlx::query(
                "UPDATE local_sessions SET state='stopped' WHERE session_id=? AND instance_id=?",
            )
            .bind(record.request.target.session_id.to_string())
            .bind(instance)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE command_receipts SET status='uncertain' WHERE session_id=? AND status IN ('queued','executing')")
                .bind(record.request.target.session_id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Preserve unknown effects when a process exits or a launch cannot complete.
    ///
    /// # Errors
    /// Reports journal errors. Stale instances cannot modify a newer process.
    pub async fn interrupted(
        &self,
        id: SessionId,
        instance: &str,
        reason: &str,
    ) -> Result<(), NodeError> {
        self.finish_session(id, instance, SessionStatus::Interrupted, reason)
            .await
    }

    /// Record confirmed process shutdown; queued/unfinished work stays uncertain.
    ///
    /// # Errors
    /// Reports journal errors without discarding outstanding command records.
    pub async fn stopped(&self, id: SessionId, instance: &str) -> Result<(), NodeError> {
        self.finish_session(id, instance, SessionStatus::Stopped, "worker stopped")
            .await
    }

    async fn finish_session(
        &self,
        id: SessionId,
        instance: &str,
        state: SessionStatus,
        reason: &str,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query("UPDATE local_sessions SET state=?,last_error=? WHERE session_id=? AND instance_id=? AND state <> 'stopped'")
            .bind(state).bind(reason).bind(id.to_string()).bind(instance).execute(&mut *tx).await?.rows_affected();
        if changed != 0 {
            sqlx::query("UPDATE command_receipts SET status='uncertain' WHERE session_id=? AND status IN ('queued','executing')")
                .bind(id.to_string()).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Mark missed heartbeats conservatively; never spawn a replacement here.
    ///
    /// # Errors
    /// Rejects a negative timeout and reports storage errors.
    pub async fn expire(&self, timeout_ms: i64) -> Result<(), NodeError> {
        if timeout_ms < 0 {
            return Err(NodeError::Rejected("heartbeat timeout cannot be negative"));
        }
        // Selection and updates share a write transaction, so a fresh heartbeat
        // cannot be overwritten between checking its timestamp and expiring it.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE local_sessions SET state='interrupted',last_error='heartbeat expired' WHERE state IN ('starting','running') AND heartbeat_at <= CAST(unixepoch('subsec')*1000 AS INTEGER)-?")
            .bind(timeout_ms).execute(&mut *tx).await?;
        sqlx::query("UPDATE command_receipts SET status='uncertain' WHERE status IN ('queued','executing') AND session_id IN (SELECT session_id FROM local_sessions WHERE state='interrupted')")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Request all workers to stop through their journal, never by a saved PID.
    ///
    /// # Errors
    /// Reports unavailable storage.
    pub async fn request_stop_all(&self) -> Result<(), NodeError> {
        sqlx::query("UPDATE local_sessions SET stop_requested=1 WHERE state <> 'stopped'")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Read an ordered, bounded batch of unacknowledged results.
    ///
    /// # Errors
    /// Reports unavailable or corrupt storage.
    pub async fn pending_events(&self) -> Result<Vec<PendingEvent>, NodeError> {
        Ok(sqlx::query_as("SELECT sequence,command_id,outcome FROM pending_events WHERE acknowledged=0 ORDER BY sequence LIMIT 100")
            .fetch_all(&self.pool).await?)
    }

    /// Build a bounded report from durable local records. Paths stay local.
    ///
    /// # Errors
    /// Reports storage failures or corrupt serialized records.
    pub async fn sync_report(
        &self,
        profile: orchestrator_protocol::sync::NodeProfile,
    ) -> Result<orchestrator_protocol::sync::SyncRequest, NodeError> {
        use orchestrator_protocol::sync::{CommandOutcome, SyncEvent, SyncRequest};
        let rows: Vec<(i64, Json<AgentCommand>, Json<CommandOutcome>)> = sqlx::query_as("SELECT e.sequence,c.request,e.outcome FROM pending_events e JOIN command_receipts c ON c.command_id=e.command_id WHERE e.acknowledged=0 ORDER BY e.sequence LIMIT 4")
            .fetch_all(&self.pool).await?;
        let active_sessions: u32 =
            sqlx::query_scalar("SELECT count(*) FROM local_sessions WHERE state <> 'stopped'")
                .fetch_one(&self.pool)
                .await?;
        Ok(SyncRequest {
            version: orchestrator_protocol::values::ProtocolVersion::V1,
            group_id: self.identity.group_id,
            node_id: self.identity.node_id,
            profile,
            active_sessions,
            events: rows
                .into_iter()
                .map(|(sequence, command, outcome)| SyncEvent {
                    sequence,
                    command: command.0,
                    outcome: outcome.0,
                })
                .collect(),
        })
    }

    /// Acknowledge exactly one known result; repeated acknowledgments are safe.
    ///
    /// # Errors
    /// Reports unavailable storage.
    pub async fn acknowledge(&self, sequence: i64) -> Result<bool, NodeError> {
        Ok(
            sqlx::query("UPDATE pending_events SET acknowledged=1 WHERE sequence=?")
                .bind(sequence)
                .execute(&self.pool)
                .await?
                .rows_affected()
                != 0,
        )
    }

    /// Read a consistent readiness and acceptance snapshot. Local paths stay local.
    ///
    /// # Errors
    /// Reports unavailable or corrupt storage.
    pub async fn dispatch_poll(
        &self,
    ) -> Result<orchestrator_protocol::routing::DispatchPoll, NodeError> {
        let mut tx = self.pool.begin().await?;
        let workspaces: Vec<Json<WorkspaceId>> = sqlx::query_scalar("SELECT json_quote(workspace_id) FROM workspace_mounts ORDER BY workspace_id LIMIT 1024").fetch_all(&mut *tx).await?;
        let accepted: Vec<Json<CommandId>> = sqlx::query_scalar(
            "SELECT json_quote(command_id) FROM dispatch_receipts ORDER BY command_id LIMIT 32",
        )
        .fetch_all(&mut *tx)
        .await?;
        let active_sessions =
            sqlx::query_scalar("SELECT count(*) FROM local_sessions WHERE state <> 'stopped'")
                .fetch_one(&mut *tx)
                .await?;
        Ok(orchestrator_protocol::routing::DispatchPoll {
            workspaces: workspaces.into_iter().map(|id| id.0).collect(),
            accepted: accepted.into_iter().map(|id| id.0).collect(),
            active_sessions,
        })
    }

    /// Save delivery acknowledgment only after the command has a durable receipt.
    ///
    /// # Errors
    /// Reports storage failures, including a missing accepted command.
    pub async fn received_dispatch(&self, command: CommandId) -> Result<(), NodeError> {
        sqlx::query("INSERT INTO dispatch_receipts(command_id) VALUES (?) ON CONFLICT DO NOTHING")
            .bind(command.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Retire an exact batch of delivery receipts after the coordinator commits it.
    ///
    /// # Errors
    /// Reports storage failures; the batch is removed atomically.
    pub async fn acknowledge_dispatch(&self, commands: &[CommandId]) -> Result<(), NodeError> {
        let mut tx = self.pool.begin().await?;
        for command in commands {
            sqlx::query("DELETE FROM dispatch_receipts WHERE command_id=?")
                .bind(command.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn connect(path: &Path, create: bool) -> Result<SqlitePool, NodeError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5));
    Ok(SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?)
}

async fn read_identity(pool: &SqlitePool) -> Result<NodeIdentity, NodeError> {
    let Json(identity) = sqlx::query_scalar("SELECT identity FROM node_identity WHERE singleton=1")
        .fetch_one(pool)
        .await?;
    Ok(identity)
}

fn ensure_changed(rows: u64) -> Result<(), NodeError> {
    if rows == 0 {
        return Err(NodeError::Rejected("worker no longer owns this session"));
    }
    Ok(())
}

fn utf8_path(path: &Path) -> Result<&str, NodeError> {
    path.to_str()
        .ok_or(NodeError::Rejected("workspace path must be UTF-8"))
}

fn canonical_workspace(path: &Path) -> Result<String, NodeError> {
    utf8_path(path)?;
    std::fs::canonicalize(path)
        .map_err(NodeError::from)
        .and_then(|canonical| {
            if !canonical.is_dir() {
                return Err(NodeError::Rejected("workspace must be a directory"));
            }
            utf8_path(&canonical).map(str::to_owned)
        })
}

fn validate_outcome(command: &AgentCommand, outcome: &CommandOutcome) -> Result<(), NodeError> {
    if let CommandOutcome::Completed { event } = outcome {
        let same_address = event.version == command.version
            && event.command_id == command.command_id
            && event.group_id == command.group_id
            && event.node_id == command.node_id
            && event.target == command.target;
        let same_action = match (&command.action, &event.kind) {
            (
                AgentAction::Start { message, .. } | AgentAction::Send { message },
                AgentEventKind::Replied { message_id, .. },
            ) => message.id == *message_id,
            (AgentAction::Cancel, AgentEventKind::Cancelled) => true,
            _ => false,
        };
        if !same_address || !same_action {
            return Err(NodeError::Rejected("adapter returned a mismatched result"));
        }
    }
    Ok(())
}
