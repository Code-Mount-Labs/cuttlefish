//! Durable permission wait and provider response delivery. The local operator
//! enrolls separate reviewer credentials; neither a node token nor model output
//! authenticates a review. The journal and operator CLI are trusted boundaries.

use crate::{NodeError, journal::Journal};
use orchestrator_protocol::{
    ids::{CommandId, PermissionId, ReviewerId, SessionId},
    permission::{
        PermissionDecision, PermissionRecord, PermissionRequest, PermissionReview, PermissionState,
        ProviderBinding,
    },
    sync::NodeToken,
};
use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, types::Json};

const RECORD: &str = "SELECT record FROM runtime_permissions WHERE permission_id=?";

impl Journal {
    /// Page unresolved permissions by ID (32 at a time). Expired requests remain
    /// visible for reconciliation; they are never treated as implicit approval.
    ///
    /// # Errors
    /// Reports storage failure or corrupt records.
    pub async fn pending_permissions(
        &self,
        after: Option<PermissionId>,
    ) -> Result<Vec<PermissionRecord>, NodeError> {
        let records: Vec<Json<PermissionRecord>> = sqlx::query_scalar("SELECT record FROM runtime_permissions WHERE permission_id > ? AND json_extract(record,'$.state') NOT IN ('delivered','abandoned') ORDER BY permission_id LIMIT 32")
            .bind(after.map(|id| id.to_string()).unwrap_or_default()).fetch_all(&self.pool).await?;
        Ok(records.into_iter().map(|record| record.0).collect())
    }

    /// Enroll, rotate or revoke a reviewer through the trusted local operator.
    /// The token must be independent of node/provider credentials and kept out
    /// of model context. Only its domain-separated SHA-256 digest is persisted.
    ///
    /// # Errors
    /// Reports storage failure, including a credential already owned by another reviewer.
    pub async fn configure_reviewer(
        &self,
        id: ReviewerId,
        token: &NodeToken,
        enabled: bool,
    ) -> Result<(), NodeError> {
        sqlx::query("INSERT INTO permission_reviewers(reviewer_id,token_hash,enabled) VALUES(?,?,?) ON CONFLICT(reviewer_id) DO UPDATE SET token_hash=excluded.token_hash,enabled=excluded.enabled")
            .bind(id.to_string()).bind(reviewer_hash(token)).bind(enabled).execute(&self.pool).await?;
        Ok(())
    }

    /// Bind a claimed command's session to one external session, immutably.
    /// Call this before sending any effect-producing provider operation.
    ///
    /// # Errors
    /// Rejects stale workers, mismatched execution scope and changed bindings.
    pub async fn bind_provider(
        &self,
        command: CommandId,
        instance: &str,
        binding: &ProviderBinding,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        owned(&mut tx, command, instance, binding).await?;
        let previous: Option<Json<ProviderBinding>> =
            sqlx::query_scalar("SELECT binding FROM provider_bindings WHERE session_id=?")
                .bind(binding.target.session_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(previous) = previous {
            if previous.0 != *binding {
                return Err(NodeError::Rejected("provider binding is immutable"));
            }
        } else {
            sqlx::query("INSERT INTO provider_bindings(session_id,binding) VALUES(?,?)")
                .bind(binding.target.session_id.to_string())
                .bind(Json(binding))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Inspect the original provider mapping without creating or resuming work.
    ///
    /// # Errors
    /// Reports storage failure or corrupt binding data.
    pub async fn provider_binding(
        &self,
        session: SessionId,
    ) -> Result<Option<ProviderBinding>, NodeError> {
        let binding: Option<Json<ProviderBinding>> =
            sqlx::query_scalar("SELECT binding FROM provider_bindings WHERE session_id=?")
                .bind(session.to_string())
                .fetch_optional(&self.pool)
                .await?;
        Ok(binding.map(|value| value.0))
    }

    /// Save an exact permission callback before waiting for a human. Repeated
    /// identical callbacks return the existing record, including its decision.
    ///
    /// # Errors
    /// Rejects expired requests, changed IDs, missing bindings and stale workers.
    pub async fn request_permission(
        &self,
        instance: &str,
        request: &PermissionRequest,
    ) -> Result<PermissionRecord, NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        owned(&mut tx, request.command_id, instance, &request.binding).await?;
        bound(&mut tx, &request.binding).await?;
        let previous: Option<Json<PermissionRecord>> = sqlx::query_scalar(RECORD)
            .bind(request.permission_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(previous) = previous {
            if previous.request != *request {
                return Err(NodeError::Rejected("permission request is immutable"));
            }
            return Ok(previous.0);
        }
        unexpired(&mut tx, request).await?;
        let record = PermissionRecord {
            request: request.clone(),
            state: PermissionState::Waiting,
            review: None,
        };
        sqlx::query(
            "INSERT INTO runtime_permissions(permission_id,command_id,record) VALUES(?,?,?)",
        )
        .bind(request.permission_id.to_string())
        .bind(request.command_id.to_string())
        .bind(Json(&record))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Read a permission request and its immutable decision, including after restart.
    ///
    /// # Errors
    /// Reports a missing request, storage failure or corrupt data.
    pub async fn permission(&self, id: PermissionId) -> Result<PermissionRecord, NodeError> {
        let record: Json<PermissionRecord> = sqlx::query_scalar(RECORD)
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
        Ok(record.0)
    }

    /// Authenticate a human decision and commit it with trusted identity/time.
    /// Expiration is checked again before delivery. Exact retries are read-only;
    /// a different reviewer or payload cannot rewrite the first decision.
    ///
    /// # Errors
    /// Rejects unknown/revoked credentials, expiry, digest mismatch or conflicting decisions.
    pub async fn decide_permission(
        &self,
        token: &NodeToken,
        decision: &PermissionDecision,
    ) -> Result<PermissionRecord, NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let reviewer: Option<Json<ReviewerId>> = sqlx::query_scalar("SELECT json_quote(reviewer_id) FROM permission_reviewers WHERE token_hash=? AND enabled=1")
            .bind(reviewer_hash(token)).fetch_optional(&mut *tx).await?;
        let reviewer = reviewer
            .ok_or(NodeError::Rejected("unauthorized permission reviewer"))?
            .0;
        let mut record: Json<PermissionRecord> = sqlx::query_scalar(RECORD)
            .bind(decision.permission_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if record.state == PermissionState::Abandoned {
            return Err(NodeError::Rejected("permission request was abandoned"));
        }
        if let Some(review) = &record.review {
            if review.reviewer_id != reviewer || review.decision != *decision {
                return Err(NodeError::Rejected("permission decision is immutable"));
            }
            return Ok(record.0);
        }
        if decision.request_digest != record.request.digest() {
            return Err(NodeError::Rejected("permission digest mismatch"));
        }
        let now = unexpired(&mut tx, &record.request).await?;
        record.review = Some(PermissionReview {
            decision: decision.clone(),
            reviewer_id: reviewer,
            decided_at_ms: now,
        });
        record.state = PermissionState::Decided;
        save(&mut tx, &record).await?;
        tx.commit().await?;
        Ok(record.0)
    }

    /// Commit a one-shot delivery intent before replying to the provider. A lost
    /// reply leaves `Dispatching`, which always requires explicit reconciliation.
    ///
    /// # Errors
    /// Rejects missing/expired approval, revoked reviewers, stale ownership and retries.
    pub async fn begin_permission_delivery(
        &self,
        id: PermissionId,
        instance: &str,
    ) -> Result<PermissionRecord, NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut record: Json<PermissionRecord> = sqlx::query_scalar(RECORD)
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if record.state != PermissionState::Decided {
            return Err(NodeError::Rejected("permission is not ready for delivery"));
        }
        let review = record
            .review
            .as_ref()
            .ok_or(NodeError::Rejected("permission review missing"))?;
        if review.decision.request_digest != record.request.digest() {
            return Err(NodeError::Rejected("stored permission digest mismatch"));
        }
        let enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM permission_reviewers WHERE reviewer_id=? AND enabled=1)",
        )
        .bind(review.reviewer_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if !enabled {
            return Err(NodeError::Rejected("permission reviewer was revoked"));
        }
        unexpired(&mut tx, &record.request).await?;
        owned(
            &mut tx,
            record.request.command_id,
            instance,
            &record.request.binding,
        )
        .await?;
        bound(&mut tx, &record.request.binding).await?;
        record.state = PermissionState::Dispatching;
        save(&mut tx, &record).await?;
        tx.commit().await?;
        Ok(record.0)
    }

    /// Withdraw a request before delivery, including an expired request. The
    /// adapter may request fresh approval with a new identity afterward. This
    /// cannot clear an ambiguous external effect or authorize any provider reply.
    ///
    /// # Errors
    /// Rejects stale workers and requests whose response was already attempted.
    pub async fn abandon_permission(
        &self,
        id: PermissionId,
        instance: &str,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut record: Json<PermissionRecord> = sqlx::query_scalar(RECORD)
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        owned(
            &mut tx,
            record.request.command_id,
            instance,
            &record.request.binding,
        )
        .await?;
        if matches!(
            record.state,
            PermissionState::Dispatching | PermissionState::Delivered
        ) {
            return Err(NodeError::Rejected(
                "attempted permission delivery requires reconciliation",
            ));
        }
        record.state = PermissionState::Abandoned;
        save(&mut tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record a provider's positive acknowledgment, never infer it from timeout.
    /// This is idempotent and does not complete the containing command.
    ///
    /// # Errors
    /// Rejects a different worker, scope or unattempted delivery; reports storage failure.
    pub async fn acknowledge_permission(
        &self,
        id: PermissionId,
        instance: &str,
    ) -> Result<(), NodeError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut record: Json<PermissionRecord> = sqlx::query_scalar(RECORD)
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        owned(
            &mut tx,
            record.request.command_id,
            instance,
            &record.request.binding,
        )
        .await?;
        if !matches!(
            record.state,
            PermissionState::Dispatching | PermissionState::Delivered
        ) {
            return Err(NodeError::Rejected("permission delivery was not attempted"));
        }
        record.state = PermissionState::Delivered;
        save(&mut tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

fn reviewer_hash(token: &NodeToken) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"cuttlefish.permission-reviewer.v1\0");
    hash.update(token.expose().as_bytes());
    hash.finalize().to_vec()
}

async fn owned(
    connection: &mut SqliteConnection,
    command: CommandId,
    instance: &str,
    binding: &ProviderBinding,
) -> Result<(), NodeError> {
    use orchestrator_protocol::agent::AgentCommand;
    let saved: Option<Json<AgentCommand>> = sqlx::query_scalar("SELECT c.request FROM command_receipts c JOIN local_sessions s ON s.session_id=c.session_id WHERE c.command_id=? AND c.status IN ('executing','uncertain') AND s.instance_id=? AND s.state='running' AND s.stop_requested=0")
        .bind(command.to_string()).bind(instance).fetch_optional(connection).await?;
    let saved = saved.ok_or(NodeError::Rejected("worker does not own an active command"))?;
    if saved.group_id != binding.group_id
        || saved.node_id != binding.node_id
        || saved.target != binding.target
    {
        return Err(NodeError::Rejected("provider execution scope mismatch"));
    }
    Ok(())
}

async fn bound(
    connection: &mut SqliteConnection,
    binding: &ProviderBinding,
) -> Result<(), NodeError> {
    let saved: Json<ProviderBinding> =
        sqlx::query_scalar("SELECT binding FROM provider_bindings WHERE session_id=?")
            .bind(binding.target.session_id.to_string())
            .fetch_one(connection)
            .await?;
    if saved.0 != *binding {
        return Err(NodeError::Rejected("provider binding mismatch"));
    }
    Ok(())
}

async fn unexpired(
    connection: &mut SqliteConnection,
    request: &PermissionRequest,
) -> Result<i64, NodeError> {
    let now: i64 = sqlx::query_scalar("SELECT CAST(unixepoch('subsec')*1000 AS INTEGER)")
        .fetch_one(connection)
        .await?;
    if request.expires_at_ms <= now {
        return Err(NodeError::Rejected(
            "permission expired; request fresh approval",
        ));
    }
    Ok(now)
}

async fn save(
    connection: &mut SqliteConnection,
    record: &PermissionRecord,
) -> Result<(), NodeError> {
    sqlx::query("UPDATE runtime_permissions SET record=? WHERE permission_id=?")
        .bind(Json(record))
        .bind(record.request.permission_id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/internal/permissions.rs"]
pub(crate) mod tests;
