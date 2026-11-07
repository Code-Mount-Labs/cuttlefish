//! Outbound directory registration and enrollment. Database credentials never
//! appear in these requests; only per-installation transport credentials do.
use crate::{Error, settings::validate_origin, state::Machine};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationHeartbeat, InstallationRole, Registration},
    sync::NodeToken,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

/// Installation choices supplied by the hosted group registry.
#[derive(Debug, Deserialize)]
pub struct Topology {
    /// Currently available roles.
    pub roles: Vec<InstallationRole>,
    /// Existing router origin, if assigned.
    pub router_url: Option<String>,
}
/// Bounded JSON client for an explicitly configured registry or router origin.
#[derive(Debug, Clone)]
pub struct Client {
    client: reqwest::Client,
    origin: String,
}
impl Client {
    /// Create an HTTPS client; literal loopback HTTP is allowed for local setups.
    ///
    /// # Errors
    /// Rejects unsafe origins or inability to initialize TLS.
    pub fn new(origin: &str) -> Result<Self, Error> {
        let origin = validate_origin(origin)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Dependency("network_client_unavailable"))?;
        Ok(Self { client, origin })
    }
    /// Post one bounded JSON request without following redirects or logging secrets.
    ///
    /// # Errors
    /// Reports transport failures, rejected requests and malformed replies.
    pub async fn post<I: Serialize + ?Sized, O: DeserializeOwned>(
        &self,
        path: &str,
        token: Option<&NodeToken>,
        input: &I,
    ) -> Result<O, Error> {
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['\\', '\r', '\n']) {
            return Err(Error::Invalid("invalid_request_path"));
        }
        let mut request = self
            .client
            .post(format!("{}{path}", self.origin))
            .json(input);
        if let Some(token) = token {
            request = request.bearer_auth(token.expose());
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| Error::Dependency("connection_failed"))?;
        if !response.status().is_success() {
            return Err(Error::Dependency("remote_request_rejected"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::Dependency("response_interrupted"))?
        {
            if bytes.len() + chunk.len() > 65_536 {
                return Err(Error::Dependency("response_too_large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Dependency("invalid_remote_response"))
    }
    /// Validate a one-use invitation and read the group's current topology.
    ///
    /// # Errors
    /// Reports expired invitations, unavailable registry, or invalid response.
    pub async fn preflight(
        &self,
        group: GroupId,
        invitation: &NodeToken,
    ) -> Result<Topology, Error> {
        self.post(
            "/api/v1/installations/preflight",
            None,
            &serde_json::json!({"group_id":group,"invitation":invitation.expose()}),
        )
        .await
    }
}
#[derive(Deserialize)]
struct Reply {
    group_id: GroupId,
}
#[derive(Deserialize)]
struct RegistrationReply {
    group_id: GroupId,
    installation_id: InstallationId,
    router_url: String,
}
impl Machine {
    /// Register the previously saved exact identity, then retain its router address.
    /// Retry after a lost response sends the original registration unchanged.
    ///
    /// # Errors
    /// Reports unavailable registry, invalid replies, or local journal failures.
    pub async fn register(&self) -> Result<(), Error> {
        let identity = self.identity().await?;
        let client = Client::new(&identity.platform_origin)?;
        let reply: RegistrationReply = client
            .post("/api/v1/installations", None, &identity.registration)
            .await?;
        if reply.group_id != identity.registration.group_id
            || reply.installation_id != identity.registration.installation_id
        {
            return Err(Error::Dependency("registration_identity_mismatch"));
        }
        let origin = validate_origin(&reply.router_url)?;
        self.save_setting("router_origin", &origin).await?;
        self.save_setting("registered", "true").await
    }
    /// Advertise readiness and dashboard links without publishing database secrets.
    ///
    /// # Errors
    /// Reports registry or local journal failure. Execution remains independent.
    pub async fn advertise(&self, agent_ready: bool) -> Result<(), Error> {
        if self.setting("registered").await?.is_none() {
            self.register().await?;
        }
        let identity = self.identity().await?;
        let role = identity.registration.role;
        let router_ready = !role.has_router() || self.status().await?.state == "ready";
        let agent_ready = !role.has_agent() || agent_ready;
        let heartbeat = InstallationHeartbeat {
            ready: router_ready && agent_ready,
            router_dashboard_url: role.has_router().then(|| identity.public_origin.clone()),
            agent_dashboard_url: role.has_agent().then_some(identity.public_origin),
        };
        let _: serde_json::Value = Client::new(&identity.platform_origin)?
            .post(
                "/api/v1/installations/heartbeat",
                Some(&identity.registration.credential),
                &heartbeat,
            )
            .await?;
        Ok(())
    }
    /// Read the assigned router without exposing the database connection string.
    ///
    /// # Errors
    /// Reports pending registration or local storage failure.
    pub async fn router_origin(&self) -> Result<String, Error> {
        self.setting("router_origin")
            .await?
            .ok_or(Error::Invalid("registration_pending"))
    }
    /// Verify an agent through this router's group-scoped directory credential.
    ///
    /// # Errors
    /// Rejects other groups, wrong roles, and unavailable authorization service.
    pub async fn authorize_agent(
        &self,
        installation: InstallationId,
        credential: &NodeToken,
    ) -> Result<GroupId, Error> {
        let identity = self.identity().await?;
        if !identity.registration.role.has_router() {
            return Err(Error::Unauthorized);
        }
        let reply:Reply=Client::new(&identity.platform_origin)?.post("/api/v1/router/authorize-agent",Some(&identity.registration.credential),&serde_json::json!({"installation_id":installation,"credential":credential.expose()})).await?;
        if reply.group_id != identity.registration.group_id {
            return Err(Error::Unauthorized);
        }
        Ok(reply.group_id)
    }
}
/// Authenticate an enrollment request at a group's installed router.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    /// Stable installation identity.
    pub installation_id: InstallationId,
    /// Per-installation credential, not a database password.
    pub credential: String,
}
impl std::fmt::Debug for Enrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Enrollment([redacted])")
    }
}
impl From<&Registration> for Enrollment {
    fn from(value: &Registration) -> Self {
        Self {
            installation_id: value.installation_id,
            credential: value.credential.expose().to_owned(),
        }
    }
}
