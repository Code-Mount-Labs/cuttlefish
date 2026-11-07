//! Authenticated, revisioned address changes independent of provider choice.
use crate::{Error, Platform, digest};
use orchestrator_protocol::{remote::AddressUpdate, sync::NodeToken};

impl Platform {
    /// Change only the caller's own address, accepting exact retries and rejecting
    /// stale revisions. Changing an address does not transfer the router role.
    ///
    /// # Errors
    /// Rejects unauthenticated calls, invalid origins and conflicting revisions.
    pub async fn update_address(
        &self,
        token: &NodeToken,
        update: &AddressUpdate,
    ) -> Result<(), Error> {
        if update.revision <= 0 {
            return Err(Error::Invalid("address_revision_must_be_positive"));
        }
        let origin = orchestrator_protocol::remote::canonical_endpoint(&update.origin)
            .ok_or(Error::Invalid("invalid_public_address"))?;
        let mut tx = self.pool.begin().await?;
        let row: Option<(uuid::Uuid, i64, Option<String>)> = sqlx::query_as(
            "SELECT installation_id,address_revision,address_origin FROM installations WHERE token_hash=$1 FOR UPDATE",
        )
        .bind(digest(token.expose()))
        .fetch_optional(&mut *tx)
        .await?;
        let (id, revision, saved) = row.ok_or(Error::Unauthorized)?;
        if origin.contains("/i/") {
            if origin != format!("{}/i/{id}", self.relay_keys()?.relay) {
                return Err(Error::Unauthorized);
            }
            self.relay_epoch(id).await?;
        }
        if update.revision < revision
            || (update.revision == revision && saved.as_deref() != Some(origin.as_str()))
        {
            return Err(Error::Conflict("address_revision_conflict"));
        }
        if update.revision > revision {
            sqlx::query("UPDATE installations SET address_revision=$1,address_origin=$2,ready=FALSE,router_url=CASE WHEN role IN ('router','router_agent') THEN $2 ELSE NULL END,router_dashboard_url=CASE WHEN role IN ('router','router_agent') THEN $2 ELSE NULL END,agent_dashboard_url=CASE WHEN role IN ('agent','router_agent') THEN $2 ELSE NULL END WHERE token_hash=$3")
                .bind(update.revision).bind(&origin).bind(digest(token.expose()))
                .execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Resolve the current router address for the authenticated installation's group.
    ///
    /// # Errors
    /// Rejects unknown installation credentials or unavailable group routers.
    pub async fn router_address(&self, token: &NodeToken) -> Result<String, Error> {
        sqlx::query_scalar("SELECT router.router_url FROM installations caller JOIN installations router ON router.group_id=caller.group_id AND router.role IN ('router','router_agent') WHERE caller.token_hash=$1")
            .bind(digest(token.expose())).fetch_optional(&self.pool).await?
            .ok_or(Error::Unauthorized)
    }
}
