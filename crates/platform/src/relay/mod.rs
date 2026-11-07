//! Installation-scoped, revocable authorization for the separately hosted relay.
pub mod gateway;
pub mod keys;
pub mod plugin;
pub mod server;
pub mod web;

use crate::{Error, Platform, digest};
use orchestrator_protocol::{
    ids::InstallationId,
    remote::{RelayLease, RelayMutation},
    sync::NodeToken,
};
use std::sync::Arc;
use uuid::Uuid;

impl Platform {
    /// Enable hosted signing only when all three operator settings are supplied.
    ///
    /// # Errors
    /// Rejects incomplete configuration or invalid key material.
    pub fn configure_relay(
        self,
        relay: Option<&str>,
        public: Option<&str>,
        private: Option<&str>,
    ) -> Result<Self, Error> {
        match (relay, public, private) {
            (None, None, None) => Ok(self),
            (Some(relay), Some(public), Some(private)) => {
                let keys = keys::Keys::new(&self.origin, relay, public, Some(private))?;
                Ok(self.with_relay(keys))
            }
            _ => Err(Error::Invalid("incomplete_relay_configuration")),
        }
    }

    /// Enable relay authorization on this replica using externally managed keys.
    #[must_use]
    pub fn with_relay(mut self, keys: keys::Keys) -> Self {
        self.relay_keys = Some(Arc::new(keys));
        self
    }
    pub(super) fn relay_keys(&self) -> Result<&keys::Keys, Error> {
        self.relay_keys
            .as_deref()
            .ok_or(Error::Invalid("managed_relay_not_configured"))
    }
    /// Persist an exact connector grant or revocation before returning connection data.
    ///
    /// # Errors
    /// Rejects stale/conflicting changes, invalid revisions or unavailable storage.
    pub async fn change_relay(
        &self,
        token: &NodeToken,
        input: &RelayMutation,
    ) -> Result<RelayLease, Error> {
        let own = self.installation(token).await?;
        let keys = self.relay_keys()?;
        if input.revision <= 0 {
            return Err(Error::Invalid("relay_revision_must_be_positive"));
        }
        let id = Uuid::from(own.installation_id);
        let hash = input
            .credential
            .as_ref()
            .map(|credential| {
                NodeToken::try_from(credential.clone())
                    .map(|token| digest(token.expose()))
                    .map_err(|_| Error::Invalid("invalid_connector_credential"))
            })
            .transpose()?;
        // One statement serializes concurrent retries and preserves the epoch on exact replay.
        let epoch: Option<Uuid> = sqlx::query_scalar("INSERT INTO relay_grants(installation_id,revision,epoch,secret_hash) VALUES($1,$2,$3,$4) ON CONFLICT(installation_id) DO UPDATE SET revision=EXCLUDED.revision,secret_hash=EXCLUDED.secret_hash,epoch=CASE WHEN relay_grants.revision=EXCLUDED.revision THEN relay_grants.epoch ELSE EXCLUDED.epoch END WHERE relay_grants.revision<EXCLUDED.revision OR (relay_grants.revision=EXCLUDED.revision AND relay_grants.secret_hash IS NOT DISTINCT FROM EXCLUDED.secret_hash) RETURNING epoch")
            .bind(id).bind(input.revision).bind(Uuid::new_v4()).bind(hash).fetch_optional(&self.pool).await?;
        let epoch = epoch.ok_or(Error::Conflict("relay_revision_conflict"))?;
        Ok(RelayLease {
            installation_id: own.installation_id,
            epoch,
            public_url: format!("{}/i/{id}", keys.relay),
            issuer: keys.issuer.clone(),
        })
    }
    /// Mint a five-minute token from the installation's separate connector secret.
    ///
    /// # Errors
    /// Rejects revoked or incorrect credentials, missing signer, or unavailable storage.
    pub async fn relay_token(
        &self,
        id: InstallationId,
        secret: &NodeToken,
    ) -> Result<String, Error> {
        let epoch: Option<Uuid> = sqlx::query_scalar(
            "SELECT epoch FROM relay_grants WHERE installation_id=$1 AND secret_hash=$2",
        )
        .bind(Uuid::from(id))
        .bind(digest(secret.expose()))
        .fetch_optional(&self.pool)
        .await?;
        self.relay_keys()?.issue(
            Uuid::from(id),
            epoch.ok_or(Error::Unauthorized)?,
            jsonwebtoken::get_current_timestamp(),
        )
    }
    pub(crate) async fn relay_epoch(&self, id: Uuid) -> Result<Uuid, Error> {
        sqlx::query_scalar(
            "SELECT epoch FROM relay_grants WHERE installation_id=$1 AND secret_hash IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(Error::Unauthorized)
    }
}
