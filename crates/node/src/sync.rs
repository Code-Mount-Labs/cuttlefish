//! Outbound HTTPS synchronization. Local events survive every failed request.

use crate::{NodeError, journal::Journal, process::Launcher, runtime::NodeRuntime};
use orchestrator_protocol::routing::DispatchBatch;
use orchestrator_protocol::sync::{NodeProfile, NodeToken, SyncResponse};
use reqwest::{Client, ClientBuilder, Url};
use std::time::Duration;

/// An authenticated connection to an explicitly configured coordinator origin.
#[derive(Debug)]
pub struct Remote {
    pub(crate) client: Client,
    pub(crate) endpoint: Url,
    pub(crate) token: NodeToken,
}

impl Remote {
    /// Run one delivery cycle. A repeated network delivery cannot spawn twice.
    ///
    /// # Errors
    /// Reports transport, journal, malformed acknowledgment or local launch failures.
    pub async fn dispatch<L: Launcher>(
        &self,
        runtime: &mut NodeRuntime<L>,
    ) -> Result<(), NodeError> {
        let poll = runtime.journal().dispatch_poll().await?;
        let mut endpoint = self.endpoint.clone();
        endpoint.set_path(&format!(
            "{}/v1/dispatch/poll",
            self.endpoint.path().trim_end_matches("/v1/sync")
        ));
        let mut response = self
            .client
            .post(endpoint)
            .bearer_auth(self.token.expose())
            .json(&poll)
            .send()
            .await?
            .error_for_status()?;
        if !response.status().is_success() {
            return Err(NodeError::Rejected("unexpected dispatch HTTP status"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 1_048_576 {
                return Err(NodeError::Rejected("dispatch response exceeds 1 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let batch: DispatchBatch = serde_json::from_slice(&bytes)?;
        if batch.acknowledged != poll.accepted {
            return Err(NodeError::Rejected(
                "coordinator acknowledged different deliveries",
            ));
        }
        runtime
            .journal()
            .acknowledge_dispatch(&poll.accepted)
            .await?;
        if let Some(command) = batch.command {
            self.download_inputs(runtime.journal(), &command).await?;
            runtime.submit(&command).await?;
            runtime
                .journal()
                .received_dispatch(command.command_id)
                .await?;
        }
        Ok(())
    }

    /// Supervise local execution while exchanging work with the separate router.
    /// Shutdown leaves independent specialist processes alive.
    ///
    /// # Errors
    /// Reports shutdown signal failure. Network and journal failures are retried.
    pub async fn execute<L: Launcher>(
        &self,
        runtime: &mut NodeRuntime<L>,
        profile: &NodeProfile,
        shutdown: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<(), NodeError> {
        tokio::pin!(shutdown);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut shutdown => return result.map_err(NodeError::from),
                _ = interval.tick() => {
                    if self.cycle(runtime, profile).await.is_err() {
                        eprintln!("Node delivery cycle failed; retained local state and will retry.");
                    }
                }
            }
        }
    }

    /// Reconcile workers, upload results, then advertise readiness and receive work.
    ///
    /// # Errors
    /// Stops the cycle on any local, transport or synchronization error.
    pub async fn cycle<L: Launcher>(
        &self,
        runtime: &mut NodeRuntime<L>,
        profile: &NodeProfile,
    ) -> Result<(), NodeError> {
        runtime.tick().await?;
        self.synchronize(runtime.journal(), profile).await?;
        self.dispatch(runtime).await
    }
    /// Keep presence and results synchronized, retrying failures every five seconds.
    /// Shutdown cancels only synchronization; specialists continue independently.
    ///
    /// # Errors
    /// Reports a failure of the supplied shutdown signal. Failed uploads stay local.
    pub async fn follow(
        &self,
        journal: &Journal,
        profile: &NodeProfile,
        shutdown: impl std::future::Future<Output = std::io::Result<()>>,
    ) -> Result<(), NodeError> {
        tokio::pin!(shutdown);
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut shutdown => return result.map_err(NodeError::from),
                _ = interval.tick() => {
                    if self.synchronize(journal, profile).await.is_err() {
                        eprintln!("Node synchronization failed; retained local events and will retry.");
                    }
                }
            }
        }
    }
    /// Build a bounded client. HTTP is permitted only for literal loopback IPs.
    /// Redirects are forbidden so credentials cannot move to another origin.
    ///
    /// # Errors
    /// Rejects invalid/insecure origins or HTTP client initialization failure.
    pub fn new(origin: &str, token: &NodeToken) -> Result<Self, NodeError> {
        Self::with_builder(origin, token, Client::builder())
    }

    fn with_builder(
        origin: &str,
        token: &NodeToken,
        builder: ClientBuilder,
    ) -> Result<Self, NodeError> {
        let mut endpoint = orchestrator_protocol::remote::endpoint_url(origin)
            .ok_or(NodeError::Rejected("invalid coordinator endpoint"))?;
        endpoint.set_path(&format!(
            "{}/v1/sync",
            endpoint.path().trim_end_matches('/')
        ));
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            token: token.clone(),
        })
    }

    /// Upload one durable batch and acknowledge only its exact committed IDs.
    ///
    /// # Errors
    /// Reports journal/transport failures, invalid reports, or unexpected acknowledgments.
    pub async fn synchronize(
        &self,
        journal: &Journal,
        profile: &NodeProfile,
    ) -> Result<usize, NodeError> {
        self.upload_activity(journal).await?;
        self.upload_artifacts(journal).await?;
        let report = journal.sync_report(profile.clone()).await?;
        report.validate()?;
        let expected: Vec<i64> = report.events.iter().map(|e| e.sequence).collect();
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(self.token.expose())
            .json(&report)
            .send()
            .await?
            .error_for_status()?;
        if !response.status().is_success() {
            return Err(NodeError::Rejected("unexpected coordinator HTTP status"));
        }
        // Bound response memory even if the peer streams a body without Content-Length.
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 4096 {
                return Err(NodeError::Rejected(
                    "coordinator acknowledgment is too large",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let reply: SyncResponse = serde_json::from_slice(&bytes)?;
        if reply.acknowledged != expected {
            return Err(NodeError::Rejected(
                "coordinator acknowledged a different batch",
            ));
        }
        for sequence in &expected {
            if !journal.acknowledge(*sequence).await? {
                return Err(NodeError::Rejected("acknowledged local event disappeared"));
            }
        }
        Ok(expected.len())
    }
}

#[cfg(test)]
#[path = "../tests/internal/sync.rs"]
mod tests;
