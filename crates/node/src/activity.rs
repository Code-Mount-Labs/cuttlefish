//! Crash-consistent execution feedback, independent of the result outbox.
use crate::{NodeError, journal::Journal};
use orchestrator_protocol::{
    activity::{ActivityPhase, ExecutionActivity},
    execution::LocalProvider,
    ids::CommandId,
};
use sqlx::types::Json;

type ActivityRow = (i64, String, Json<LocalProvider>, Json<ActivityPhase>, i64);

impl Journal {
    /// Record an observed stage for the worker that currently owns this command.
    /// Consecutive duplicates are suppressed; a turn retains at most 256 events.
    ///
    /// # Errors
    /// Rejects stale workers, unclaimed commands and storage failures.
    pub async fn record_activity(
        &self,
        command: CommandId,
        instance: &str,
        provider: LocalProvider,
        phase: ActivityPhase,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM command_receipts c JOIN local_sessions s USING(session_id) WHERE c.command_id=? AND c.status='executing' AND s.instance_id=? AND s.state='running')")
            .bind(command.to_string()).bind(instance).fetch_one(&mut *tx).await?;
        if !owned {
            return Err(NodeError::Rejected(
                "activity requires the current executing worker",
            ));
        }
        sqlx::query("INSERT INTO execution_activity(command_id,provider,phase,at_ms) SELECT ?,?,?,CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE (SELECT count(*) FROM execution_activity WHERE command_id=?)<256 AND COALESCE((SELECT phase FROM execution_activity WHERE command_id=? ORDER BY sequence DESC LIMIT 1),'')<>?")
            .bind(command.to_string()).bind(Json(provider)).bind(Json(phase)).bind(command.to_string()).bind(command.to_string()).bind(Json(phase)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Read a bounded activity outbox batch in original order.
    ///
    /// # Errors
    /// Reports storage or decoding failure without dropping observations.
    pub async fn pending_activity(&self) -> Result<Vec<ExecutionActivity>, NodeError> {
        let rows: Vec<ActivityRow> = sqlx::query_as("SELECT sequence,command_id,provider,phase,at_ms FROM execution_activity WHERE acknowledged=0 ORDER BY sequence LIMIT 64").fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|(sequence, command, provider, phase, at_ms)| {
                Ok(ExecutionActivity {
                    sequence,
                    command_id: command
                        .parse()
                        .map_err(|_| NodeError::Rejected("invalid activity command"))?,
                    provider: provider.0,
                    phase: phase.0,
                    at_ms,
                })
            })
            .collect()
    }
}
impl crate::sync::Remote {
    /// Synchronize factual activity before synchronizing completed results.
    ///
    /// # Errors
    /// A failed request or acknowledgment leaves the outbox retryable.
    pub async fn upload_activity(&self, journal: &Journal) -> Result<(), NodeError> {
        // A recovered supervisor publishes uncertainty even if the provider died
        // before it could emit its own final observation.
        sqlx::query("INSERT INTO execution_activity(command_id,provider,phase,at_ms) SELECT c.command_id,a.provider,'\"needs_attention\"',CAST(unixepoch('subsec')*1000 AS INTEGER) FROM command_receipts c JOIN execution_activity a ON a.command_id=c.command_id WHERE c.status='uncertain' AND a.sequence=(SELECT max(sequence) FROM execution_activity WHERE command_id=c.command_id) AND a.phase<>'\"needs_attention\"'")
            .execute(&journal.pool).await?;
        let events = journal.pending_activity().await?;
        if events.is_empty() {
            return Ok(());
        }
        let expected: Vec<i64> = events.iter().map(|event| event.sequence).collect();
        let response = self
            .exchange(
                "/v1/activity",
                reqwest::Method::POST,
                Some(serde_json::json!(events)),
            )
            .await?;
        let acknowledged: Vec<i64> = serde_json::from_value(response)?;
        if acknowledged != expected {
            return Err(NodeError::Rejected("activity acknowledgment mismatch"));
        }
        let mut tx = journal.pool.begin_with("BEGIN IMMEDIATE").await?;
        for sequence in expected {
            sqlx::query("UPDATE execution_activity SET acknowledged=1 WHERE sequence=?")
                .bind(sequence)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/activity.rs"]
mod tests;
