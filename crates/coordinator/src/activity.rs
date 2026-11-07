//! Tenant-scoped, immutable runtime observations for chat feedback.
use crate::{
    Error,
    store::{Store, authenticate},
};
use orchestrator_protocol::{activity::ExecutionActivity, ids::ThreadId, sync::NodeToken};
use sqlx::types::Json;
use uuid::Uuid;
impl Store {
    /// Commit a bounded batch only for dispatches assigned to this authenticated node.
    ///
    /// # Errors
    /// Rejects invalid cursors, changed observations, wrong owners and storage failures.
    pub async fn record_activity(
        &self,
        token: &NodeToken,
        events: &[ExecutionActivity],
    ) -> Result<Vec<i64>, Error> {
        let mut tx = self.pool.begin().await?;
        let (group, node) = authenticate(&mut tx, token).await?;
        if events.len() > 64 {
            return Err(Error::RoutingConflict("activity batch exceeds limits"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for event in events {
            if event.sequence <= 0 || event.at_ms < 0 || !seen.insert(event.sequence) {
                return Err(Error::RoutingConflict("invalid activity cursor"));
            }
            let saved: Option<i64> = sqlx::query_scalar("INSERT INTO execution_activity(group_id,node_id,sequence,command_id,payload) SELECT $1,$2,$3,$4,$5 WHERE EXISTS(SELECT 1 FROM dispatches WHERE group_id=$1 AND node_id=$2 AND command_id=$4 AND state<>'queued') ON CONFLICT(group_id,node_id,sequence) DO UPDATE SET payload=execution_activity.payload WHERE execution_activity.payload=excluded.payload RETURNING sequence")
                .bind(group).bind(node).bind(event.sequence).bind(Uuid::from(event.command_id)).bind(Json(event)).fetch_optional(&mut *tx).await?;
            if saved.is_none() {
                return Err(Error::RoutingConflict(
                    "activity ownership or identity conflict",
                ));
            }
        }
        tx.commit().await?;
        Ok(events.iter().map(|event| event.sequence).collect())
    }
    /// Return recent observations and node liveness for a group's conversation.
    ///
    /// # Errors
    /// Reports authentication or storage failures.
    pub async fn conversation_activity(
        &self,
        token: &NodeToken,
        thread: ThreadId,
    ) -> Result<serde_json::Value, Error> {
        let (group, _) = self.identity(token).await?;
        let events: Vec<Json<ExecutionActivity>> = sqlx::query_scalar("SELECT payload FROM (SELECT a.payload,d.sequence AS turn,a.sequence FROM execution_activity a JOIN dispatches d ON d.group_id=a.group_id AND d.command_id=a.command_id WHERE a.group_id=$1 AND d.thread_id=$2 ORDER BY d.sequence DESC,a.sequence DESC LIMIT 512) recent ORDER BY turn,sequence")
            .bind(Uuid::from(group)).bind(Uuid::from(thread)).fetch_all(&self.pool).await?;
        let last_seen: Option<i64> = sqlx::query_scalar("SELECT (extract(epoch FROM n.last_seen)*1000)::bigint FROM thread_bindings t JOIN nodes n ON n.group_id=t.group_id AND n.node_id=(t.binding->>'node_id')::uuid WHERE t.group_id=$1 AND t.thread_id=$2")
            .bind(Uuid::from(group)).bind(Uuid::from(thread)).fetch_optional(&self.pool).await?.flatten();
        Ok(
            serde_json::json!({"events":events.into_iter().map(|row|row.0).collect::<Vec<_>>(),"last_seen_ms":last_seen}),
        )
    }
}
