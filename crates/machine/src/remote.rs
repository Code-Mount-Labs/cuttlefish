//! Durable remote-access selection and recovery, independent of task execution.
use crate::{Error, state::Machine};
use orchestrator_protocol::{
    ids::InstallationId,
    remote::{AddressUpdate, RelayLease, RelayMutation, RemoteChoice, canonical_endpoint},
    sync::NodeToken,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{future::Future, pin::Pin};

/// Asynchronous operating-system connector operation.
pub type ConnectorFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
/// Process-service boundary, allowing recovery without owning a child PID.
pub trait Connector: std::fmt::Debug + Send + Sync {
    /// Ensure exactly this installation's selected connector service is running.
    fn start<'a>(
        &'a self,
        lease: &'a RelayLease,
        credential: &'a str,
        local: &'a str,
    ) -> ConnectorFuture<'a>;
    /// Disable all connector services owned by this installation.
    fn stop(&self, id: InstallationId) -> ConnectorFuture<'_>;
}
/// Secret-free status returned by the local dashboard API.
#[derive(Debug, Serialize, Deserialize)]
pub struct RemoteStatus {
    /// Monotonic desired-state revision.
    pub revision: i64,
    /// Selected exposure mode.
    pub choice: RemoteChoice,
    /// Local, `waiting_for_admin`, pending, ready or failed.
    pub state: String,
    /// Last successfully published URL.
    pub public_url: Option<String>,
    /// Redacted setup failure, if any.
    pub error_code: Option<String>,
}
#[derive(sqlx::FromRow)]
struct Saved {
    revision: i64,
    choice: sqlx::types::Json<RemoteChoice>,
    credential: Option<String>,
    lease: Option<sqlx::types::Json<RelayLease>>,
    state: String,
    public_url: Option<String>,
    local_origin: Option<String>,
    proof: String,
    error_code: Option<String>,
}
impl Saved {
    fn status(self) -> RemoteStatus {
        RemoteStatus {
            revision: self.revision,
            choice: self.choice.0,
            state: self.state,
            public_url: self.public_url,
            error_code: self.error_code,
        }
    }
}
impl Machine {
    async fn remote_saved(&self) -> Result<Saved, Error> {
        Ok(sqlx::query_as("SELECT revision,choice,credential,lease,state,public_url,local_origin,proof,error_code FROM remote_access WHERE singleton=1").fetch_one(&self.pool).await?)
    }
    /// Read durable, redacted remote-access status.
    ///
    /// # Errors
    /// Reports journal corruption or unavailability.
    pub async fn remote_status(&self) -> Result<RemoteStatus, Error> {
        Ok(self.remote_saved().await?.status())
    }
    /// Save the actual local listener address before starting reconciliation.
    ///
    /// # Errors
    /// Rejects non-loopback origins or journal failures.
    pub async fn set_local_endpoint(&self, origin: &str) -> Result<(), Error> {
        let url =
            reqwest::Url::parse(origin).map_err(|_| Error::Invalid("invalid_local_endpoint"))?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
        {
            return Err(Error::Invalid("relay_target_must_be_loopback"));
        }
        let origin = crate::settings::validate_origin(origin)?;
        sqlx::query("UPDATE remote_access SET local_origin=?1 WHERE singleton=1")
            .bind(origin)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Persist a new exposure choice before acquiring any remote resource.
    ///
    /// # Errors
    /// Rejects unsafe existing addresses, overflow and storage failures.
    pub async fn select_remote(&self, choice: RemoteChoice) -> Result<RemoteStatus, Error> {
        let _guard = self.remote_changes.lock().await;
        if let RemoteChoice::Existing { origin } = &choice {
            let checked = crate::settings::validate_origin(origin)?;
            if !checked.starts_with("https://") {
                return Err(Error::Invalid("existing_address_requires_https"));
            }
        }
        let credential = matches!(choice, RemoteChoice::Managed {})
            .then(|| NodeToken::random().expose().to_owned());
        let changed = sqlx::query("UPDATE remote_access SET revision=revision+1,choice=?1,credential=?2,state='pending',error_code=NULL,proof=?3 WHERE singleton=1 AND revision<9223372036854775807")
            .bind(sqlx::types::Json(choice)).bind(credential).bind(NodeToken::random().expose()).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Invalid("remote_revision_exhausted"));
        }
        self.remote_status().await
    }
    /// Return the installation proof used to verify a route before publishing it.
    ///
    /// # Errors
    /// Reports unavailable identity or journal.
    pub async fn remote_proof(&self) -> Result<serde_json::Value, Error> {
        Ok(
            json!({"installation_id":self.identity().await?.registration.installation_id,"proof":self.remote_saved().await?.proof}),
        )
    }
    /// Allowed extra browser origin while a remote route is being verified or used.
    ///
    /// # Errors
    /// Reports unavailable journal state.
    pub async fn remote_origin(&self) -> Result<Option<String>, Error> {
        let saved = self.remote_saved().await?;
        let value = match saved.choice.0 {
            RemoteChoice::Local {} => return Ok(None),
            RemoteChoice::Existing { origin } => Some(origin),
            RemoteChoice::Managed {} => saved.lease.map(|lease| lease.public_url.clone()),
        };
        value
            .map(|value| {
                reqwest::Url::parse(&value)
                    .map(|url| url.origin().ascii_serialization())
                    .map_err(|_| Error::Invalid("invalid_saved_remote_address"))
            })
            .transpose()
    }
    /// Reconcile the journal after process restart or temporary network failure.
    ///
    /// # Errors
    /// Reports acquisition, OS service, proof, publication and journal failures.
    pub async fn recover_remote(&self) -> Result<RemoteStatus, Error> {
        let _guard = self.remote_changes.lock().await;
        let saved = self.remote_saved().await?;
        if saved.state == "local" {
            return Ok(saved.status());
        }
        let result = self.reconcile_remote(&saved).await;
        if let Err(error) = &result {
            let code = match error {
                Error::Invalid(code) | Error::Dependency(code) => *code,
                _ => "remote_setup_failed",
            };
            sqlx::query("UPDATE remote_access SET state='failed',error_code=?1 WHERE singleton=1")
                .bind(code)
                .execute(&self.pool)
                .await?;
        }
        result?;
        self.remote_status().await
    }
    async fn reconcile_remote(&self, saved: &Saved) -> Result<(), Error> {
        if !self.initialized().await? {
            sqlx::query("UPDATE remote_access SET state='waiting_for_admin' WHERE singleton=1")
                .execute(&self.pool)
                .await?;
            return Ok(());
        }
        let identity = self.identity().await?;
        let id = identity.registration.installation_id;
        // Withdrawal must not wait for registration or network-client initialization.
        if !matches!(saved.choice.0, RemoteChoice::Managed {}) {
            self.connector.stop(id).await?;
        }
        let local = saved
            .local_origin
            .as_deref()
            .ok_or(Error::Invalid("local_listener_not_configured"))?;
        if self.setting("registered").await?.is_none() {
            self.register().await?;
        }
        let platform = (self.remote_client)(&identity.platform_origin)?;
        let url = match &saved.choice.0 {
            RemoteChoice::Managed {} => {
                let credential = saved
                    .credential
                    .as_deref()
                    .ok_or(Error::Invalid("missing_connector_credential"))?;
                let lease: RelayLease = platform
                    .post(
                        "/api/v1/installations/relay",
                        Some(&identity.registration.credential),
                        &RelayMutation {
                            revision: saved.revision,
                            credential: Some(credential.into()),
                        },
                    )
                    .await?;
                let expected = format!("/i/{id}");
                if lease.installation_id != id
                    || lease.issuer != identity.platform_origin
                    || canonical_endpoint(&lease.public_url).as_deref()
                        != Some(lease.public_url.as_str())
                    || !lease.public_url.ends_with(&expected)
                    || lease.epoch.is_nil()
                {
                    return Err(Error::Invalid("relay_lease_identity_mismatch"));
                }
                sqlx::query("UPDATE remote_access SET lease=?1 WHERE singleton=1")
                    .bind(sqlx::types::Json(&lease))
                    .execute(&self.pool)
                    .await?;
                self.connector.start(&lease, credential, local).await?;
                lease.public_url
            }
            RemoteChoice::Local {} | RemoteChoice::Existing { .. } => {
                if saved.lease.is_some() {
                    let _: RelayLease = platform
                        .post(
                            "/api/v1/installations/relay",
                            Some(&identity.registration.credential),
                            &RelayMutation {
                                revision: saved.revision,
                                credential: None,
                            },
                        )
                        .await?;
                }
                match &saved.choice.0 {
                    RemoteChoice::Existing { origin } => origin.clone(),
                    _ => local.to_owned(),
                }
            }
        };
        if !matches!(saved.choice.0, RemoteChoice::Local {}) {
            let proof: serde_json::Value = (self.remote_client)(&url)?
                .post("/api/local/relay-proof", None, &json!({}))
                .await?;
            if proof["installation_id"] != id.to_string() || proof["proof"] != saved.proof {
                return Err(Error::Invalid("remote_identity_proof_failed"));
            }
        }
        let _: serde_json::Value = platform
            .post(
                "/api/v1/installations/address",
                Some(&identity.registration.credential),
                &AddressUpdate {
                    revision: saved.revision,
                    origin: url.clone(),
                },
            )
            .await?;
        let state = if matches!(saved.choice.0, RemoteChoice::Local {}) {
            "local"
        } else {
            "ready"
        };
        sqlx::query("UPDATE remote_access SET state=?1,public_url=?2,error_code=NULL,lease=CASE WHEN ?3 THEN lease ELSE NULL END WHERE singleton=1")
            .bind(state).bind(url).bind(matches!(saved.choice.0,RemoteChoice::Managed{})).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/remote.rs"]
mod tests;
