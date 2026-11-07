//! Shared, transactional routing context. All replicas use the same database.

use crate::Error;
use orchestrator_protocol::{
    ids::{GroupId, NodeId},
    sync::{EventView, NodeProfile, NodeToken, NodeView, SyncEvent, SyncRequest, SyncResponse},
    values::ProtocolVersion,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions, types::Json};
use std::time::Duration;
use uuid::Uuid;

/// A cloneable connection pool; contains no in-memory authoritative node state.
#[derive(Debug, Clone)]
pub struct Store {
    pub(crate) pool: PgPool,
}

type NodeRow = (Uuid, Option<Json<NodeProfile>>, i64, Option<i64>);

impl Store {
    /// Connect and apply checksum-verified migrations.
    ///
    /// # Errors
    /// Reports connection, migration, or permission failures.
    pub async fn open(url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Close this replica's database connections.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Enroll through an operator-controlled boundary, never public self-registration.
    /// Existing identities cannot be silently reset or moved between groups.
    ///
    /// # Errors
    /// Reports duplicate identity/token or storage errors.
    pub async fn enroll(
        &self,
        group: GroupId,
        node: NodeId,
        token: &NodeToken,
    ) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO groups(group_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(Uuid::from(group))
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO nodes(group_id,node_id,token_hash) VALUES ($1,$2,$3)")
            .bind(Uuid::from(group))
            .bind(Uuid::from(node))
            .bind(digest(token))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Revoke a credential without removing its historical events.
    ///
    /// # Errors
    /// Reports storage failure. Returns false for an unknown identity.
    pub async fn revoke(&self, group: GroupId, node: NodeId) -> Result<bool, Error> {
        Ok(
            sqlx::query("UPDATE nodes SET revoked=TRUE WHERE group_id=$1 AND node_id=$2")
                .bind(Uuid::from(group))
                .bind(Uuid::from(node))
                .execute(&self.pool)
                .await?
                .rows_affected()
                != 0,
        )
    }

    /// Commit a complete report before acknowledging any of it.
    ///
    /// # Errors
    /// Rejects wrong credentials, cross-tenant records, malformed reports and
    /// changed event identities; any failure rolls back the entire batch.
    pub async fn sync(
        &self,
        token: &NodeToken,
        report: &SyncRequest,
    ) -> Result<SyncResponse, Error> {
        let mut tx = self.pool.begin().await?;
        let (group, node) = authenticate(&mut tx, token).await?;
        if group != Uuid::from(report.group_id) || node != Uuid::from(report.node_id) {
            return Err(Error::Unauthorized);
        }
        report.validate()?;
        // Serialize event allocation per group. BIGSERIAL alone does not order
        // commits: a cursor reader could otherwise skip a slow earlier writer.
        sqlx::query("SELECT group_id FROM groups WHERE group_id=$1 FOR UPDATE")
            .bind(group)
            .fetch_one(&mut *tx)
            .await?;
        for event in &report.events {
            crate::dispatch::record_outcome(&mut tx, group, event).await?;
            let row: Option<(Json<SyncEvent>,)> = sqlx::query_as("INSERT INTO node_events(group_id,node_id,local_sequence,command_id,payload) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (group_id,node_id,local_sequence) DO UPDATE SET payload=node_events.payload WHERE node_events.payload=EXCLUDED.payload RETURNING payload")
                .bind(group).bind(node).bind(event.sequence).bind(Uuid::from(event.command.command_id)).bind(Json(event)).fetch_optional(&mut *tx).await.map_err(event_write_error)?;
            if row.is_none() {
                return Err(Error::Conflict);
            }
        }
        sqlx::query("UPDATE nodes SET profile=$1,active_sessions=$2,last_seen=clock_timestamp() WHERE node_id=$3")
            .bind(Json(&report.profile)).bind(i64::from(report.active_sessions)).bind(node).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(SyncResponse {
            version: ProtocolVersion::V1,
            acknowledged: report.events.iter().map(|e| e.sequence).collect(),
        })
    }

    /// Read this credential's peers without exposing their secrets or local paths.
    ///
    /// # Errors
    /// Reports authentication or storage failure.
    pub async fn nodes(&self, token: &NodeToken) -> Result<Vec<NodeView>, Error> {
        let mut tx = self.pool.begin().await?;
        let (group, _) = authenticate(&mut tx, token).await?;
        let rows: Vec<NodeRow> = sqlx::query_as("SELECT node_id,profile,active_sessions,(extract(epoch FROM last_seen)*1000)::bigint FROM nodes WHERE group_id=$1 AND NOT revoked ORDER BY node_id LIMIT 1000")
            .bind(group).fetch_all(&mut *tx).await?;
        rows.into_iter()
            .map(|(node, profile, active_sessions, last_seen_ms)| {
                let node_id = NodeId::try_from(node)
                    .map_err(|_| sqlx::Error::Decode("invalid node identity".into()))?;
                Ok(NodeView {
                    node_id,
                    profile: profile.map(|p| p.0),
                    active_sessions,
                    last_seen_ms,
                })
            })
            .collect()
    }

    /// Read a bounded, ordered page of committed group events.
    ///
    /// # Errors
    /// Rejects negative cursors and reports authentication or storage failures.
    pub async fn events(&self, token: &NodeToken, after: i64) -> Result<Vec<EventView>, Error> {
        let mut tx = self.pool.begin().await?;
        let (group, _) = authenticate(&mut tx, token).await?;
        if after < 0 {
            return Err(
                orchestrator_protocol::sync::InvalidReport("cursor cannot be negative").into(),
            );
        }
        let rows: Vec<(i64, Json<SyncEvent>)> = sqlx::query_as("SELECT cursor,payload FROM node_events WHERE group_id=$1 AND cursor>$2 ORDER BY cursor LIMIT 100")
            .bind(group).bind(after).fetch_all(&mut *tx).await?;
        Ok(rows
            .into_iter()
            .map(|(cursor, event)| EventView {
                cursor,
                event: event.0,
            })
            .collect())
    }
}

fn digest(token: &NodeToken) -> Vec<u8> {
    Sha256::digest(token.expose().as_bytes()).to_vec()
}

fn event_write_error(error: sqlx::Error) -> Error {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        Error::Conflict
    } else {
        Error::Database(error)
    }
}

pub(crate) async fn authenticate(
    tx: &mut Transaction<'_, Postgres>,
    token: &NodeToken,
) -> Result<(Uuid, Uuid), Error> {
    // Prevent concurrent revocation while allowing foreign-key KEY SHARE locks
    // from another authenticated node assigning work in the same group.
    sqlx::query_as(
        "SELECT group_id,node_id FROM nodes WHERE token_hash=$1 AND NOT revoked FOR NO KEY UPDATE",
    )
    .bind(digest(token))
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(Error::Unauthorized)
}
