//! Reviewed recovery stays on the original execution node and command.
use crate::{Error, store::Store};
use orchestrator_protocol::{
    ids::{CommandId, ThreadId},
    recovery::{RecoveryAction, RecoveryJob, RecoveryState, RecoveryView},
    sync::NodeToken,
};
use sqlx::types::Json;
use uuid::Uuid;
impl Store {
    /// Read recovery requests for one group-scoped conversation.
    /// # Errors
    /// Reports authentication and storage errors.
    pub async fn recoveries(
        &self,
        token: &NodeToken,
        thread: ThreadId,
    ) -> Result<Vec<RecoveryView>, Error> {
        let (group, _) = self.identity(token).await?;
        let rows:Vec<Json<RecoveryView>>=sqlx::query_scalar("SELECT r.view FROM task_recovery r JOIN dispatches d USING(group_id,command_id) WHERE r.group_id=$1 AND d.thread_id=$2 ORDER BY d.sequence LIMIT 100")
            .bind(Uuid::from(group)).bind(Uuid::from(thread)).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|v| v.0).collect())
    }
    /// Inspect saved work or approve the exact reviewed checkpoint.
    /// # Errors
    /// Rejects active/completed tasks, stale reviews and identity reuse.
    pub async fn recover(
        &self,
        token: &NodeToken,
        thread: ThreadId,
        action: &RecoveryAction,
    ) -> Result<RecoveryView, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        let id = match action {
            RecoveryAction::Inspect { id, .. } | RecoveryAction::Continue { id, .. } => *id,
        };
        let saved:Option<(Json<RecoveryView>,bool)>=sqlx::query_as("SELECT r.view,d.state='claimed' FROM task_recovery r JOIN dispatches d USING(group_id,command_id) WHERE r.group_id=$1 AND r.request_id=$2 AND d.thread_id=$3")
            .bind(group).bind(Uuid::from(id)).bind(Uuid::from(thread)).fetch_optional(&mut *tx).await?;
        let view = match (action, saved) {
            (RecoveryAction::Inspect { command_id, .. }, Some((saved, _)))
                if saved.command_id == *command_id =>
            {
                return Ok(saved.0);
            }
            (RecoveryAction::Inspect { command_id, .. }, None) => {
                let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dispatches d WHERE d.group_id=$1 AND d.thread_id=$2 AND d.command_id=$3 AND d.state='claimed' AND (SELECT a.payload->>'phase' FROM execution_activity a WHERE a.group_id=d.group_id AND a.command_id=d.command_id ORDER BY a.sequence DESC LIMIT 1)='needs_attention' AND NOT EXISTS(SELECT 1 FROM task_recovery r WHERE r.group_id=d.group_id AND r.command_id=d.command_id AND r.view->>'state'='approved'))")
                    .bind(group).bind(Uuid::from(thread)).bind(Uuid::from(*command_id)).fetch_one(&mut *tx).await?;
                if !eligible {
                    return Err(Error::RoutingConflict(
                        "Only an interrupted task can be inspected. Wait for the owning agent to reconnect.",
                    ));
                }
                RecoveryView {
                    id,
                    command_id: *command_id,
                    state: RecoveryState::Inspecting,
                    checkpoint: None,
                    detail: None,
                }
            }
            (RecoveryAction::Continue { digest, .. }, Some((Json(mut saved), active)))
                if saved
                    .checkpoint
                    .as_ref()
                    .is_some_and(|c| c.digest == *digest) =>
            {
                if matches!(
                    saved.state,
                    RecoveryState::Approved | RecoveryState::Resumed
                ) {
                    return Ok(saved);
                }
                if saved.state != RecoveryState::Ready {
                    return Err(Error::RoutingConflict(
                        "Inspect the saved work before continuing.",
                    ));
                }
                if !active {
                    return Err(Error::RoutingConflict("This task has already finished."));
                }
                saved.state = RecoveryState::Approved;
                saved
            }
            _ => {
                return Err(Error::RoutingConflict(
                    "The recovery review changed. Inspect the saved work again.",
                ));
            }
        };
        sqlx::query("INSERT INTO task_recovery(group_id,command_id,request_id,view) VALUES($1,$2,$3,$4) ON CONFLICT(group_id,command_id) DO UPDATE SET request_id=excluded.request_id,view=excluded.view")
            .bind(group).bind(Uuid::from(view.command_id)).bind(Uuid::from(id)).bind(Json(&view)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(view)
    }
    /// Deliver one inspection or approved continuation only to its original node.
    /// # Errors
    /// Reports authentication, decoding and storage errors.
    pub async fn recovery_job(&self, token: &NodeToken) -> Result<Option<RecoveryJob>, Error> {
        let (group, node) = self.identity(token).await?;
        let row:Option<Json<RecoveryJob>>=sqlx::query_scalar("SELECT jsonb_build_object('view',r.view,'command',d.command) FROM task_recovery r JOIN dispatches d USING(group_id,command_id) WHERE r.group_id=$1 AND d.node_id=$2 AND r.view->>'state' IN ('inspecting','approved') ORDER BY d.sequence LIMIT 1")
            .bind(Uuid::from(group)).bind(Uuid::from(node)).fetch_optional(&self.pool).await?;
        Ok(row.map(|v| v.0))
    }
    /// Commit a bounded, node-authenticated result without losing review identity.
    /// # Errors
    /// Rejects cross-node, stale, malformed or invalid state transitions.
    pub async fn recovery_report(
        &self,
        token: &NodeToken,
        view: &RecoveryView,
    ) -> Result<CommandId, Error> {
        if serde_json::json!(view).to_string().len() > 8192 {
            return Err(Error::RoutingConflict("Recovery report exceeds limits."));
        }
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        let saved:Option<Json<RecoveryView>>=sqlx::query_scalar("SELECT r.view FROM task_recovery r JOIN dispatches d USING(group_id,command_id) WHERE r.group_id=$1 AND d.node_id=$2 AND r.command_id=$3 AND r.request_id=$4")
            .bind(group).bind(node).bind(Uuid::from(view.command_id)).bind(Uuid::from(view.id)).fetch_optional(&mut *tx).await?;
        let saved = saved.ok_or(Error::Unauthorized)?.0;
        if saved == *view {
            return Ok(view.id);
        }
        let valid = match (saved.state, view.state) {
            (RecoveryState::Inspecting, RecoveryState::Ready) => {
                view.checkpoint.as_ref().is_some_and(|c| {
                    c.digest.len() == 64 && c.digest.bytes().all(|b| b.is_ascii_hexdigit())
                }) && view.detail.is_none()
            }
            (RecoveryState::Inspecting | RecoveryState::Approved, RecoveryState::Blocked) => view
                .detail
                .as_ref()
                .is_some_and(|s| !s.is_empty() && s.len() <= 1000),
            (RecoveryState::Approved, RecoveryState::Resumed) => {
                saved.checkpoint == view.checkpoint && view.detail.is_none()
            }
            _ => false,
        };
        if !valid {
            return Err(Error::RoutingConflict("Invalid recovery transition."));
        }
        sqlx::query("UPDATE task_recovery SET view=$1 WHERE group_id=$2 AND command_id=$3")
            .bind(Json(view))
            .bind(group)
            .bind(Uuid::from(view.command_id))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(view.id)
    }
}
