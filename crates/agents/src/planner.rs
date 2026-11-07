//! Super-agent reasoning boundary. Decisions are validated and persisted by the runtime.
use orchestrator_protocol::{
    capabilities::Capability,
    collaboration::{CapabilityRelease, GoalCommand, GoalView},
    sync::NodeView,
};
use reqwest::{Client, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fmt::Debug, future::Future, time::Duration};

/// Bounded decision context; no credentials or local file contents are included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningContext {
    /// Reviewed deployments that can be reused without another acquisition proposal.
    pub installations: Vec<orchestrator_protocol::collaboration::InstalledCapability>,
    /// Logical workspaces registered on this node; never local directory paths.
    pub workspaces: Vec<orchestrator_protocol::ids::WorkspaceId>,
    /// Authoritative goal and all child execution receipts.
    pub goal: GoalView,
    /// Peers, their capabilities and current capacity observations.
    pub peers: Vec<NodeView>,
    /// Organization-approved discovery sources and immutable harness versions.
    pub catalog: Vec<CapabilityRelease>,
    /// Planning, delegation and learning are native capabilities.
    pub native: BTreeSet<Capability>,
}
/// Reasoning does not perform execution or approve acquisitions.
pub trait Planner: Debug + Send + Sync {
    /// Decide one revision-checked operation, or wait for an external event.
    ///
    /// # Errors
    /// Reports configuration, transport or invalid model output.
    fn decide(
        &self,
        context: &PlanningContext,
    ) -> impl Future<Output = Result<Option<GoalCommand>, PlannerError>> + Send;
}
/// Deliberately redacted model errors never include credentials or response bodies.
#[derive(Debug, thiserror::Error)]
#[error("planning model unavailable or returned an invalid decision")]
pub struct PlannerError;

/// Configurable Responses API planner; no execution tools are supplied to the model.
#[derive(Debug)]
pub struct ModelPlanner {
    client: Client,
    endpoint: Url,
    authorization: HeaderValue,
    model: String,
    instructions: orchestrator_prompts::Prompt,
}
impl ModelPlanner {
    /// Configure an explicit model and full endpoint, using TLS or loopback HTTP.
    ///
    /// # Errors
    /// Rejects invalid origins, credentials and client configuration.
    pub fn new(endpoint: &str, key: &str, model: &str) -> Result<Self, PlannerError> {
        Self::with_client(endpoint, key, model, Client::builder())
    }
    /// Replace the bundled wording with a validated, operator-reviewed prompt.
    #[must_use]
    pub fn with_prompt(mut self, prompt: orchestrator_prompts::Prompt) -> Self {
        self.instructions = prompt;
        self
    }

    fn with_client(
        endpoint: &str,
        key: &str,
        model: &str,
        builder: reqwest::ClientBuilder,
    ) -> Result<Self, PlannerError> {
        let endpoint = Url::parse(endpoint).map_err(|_| PlannerError)?;
        let loopback = endpoint
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || key.trim().is_empty()
            || model.trim().is_empty()
            || model.len() > 128
        {
            return Err(PlannerError);
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| PlannerError)?;
        authorization.set_sensitive(true);
        let client = builder
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| PlannerError)?;
        Ok(Self {
            client,
            endpoint,
            authorization,
            model: model.to_owned(),
            instructions: orchestrator_prompts::Prompt::bundled(
                orchestrator_prompts::Kind::Planner,
            ),
        })
    }
}
impl Planner for ModelPlanner {
    async fn decide(&self, context: &PlanningContext) -> Result<Option<GoalCommand>, PlannerError> {
        let input = json!(context).to_string();
        if input.len() > 1_048_576 {
            return Err(PlannerError);
        }
        let body = json!({"model":self.model,"store":false,"max_output_tokens":4096,"instructions":self.instructions.as_str(),"input":input,"text":{"format":{"type":"json_object"}}});
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header("authorization", self.authorization.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| PlannerError)?;
        if !response.status().is_success() {
            return Err(PlannerError);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| PlannerError)? {
            if bytes.len() + chunk.len() > 262_144 {
                return Err(PlannerError);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| PlannerError)?;
        decode(&value)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    command: Option<GoalCommand>,
}
fn decode(value: &Value) -> Result<Option<GoalCommand>, PlannerError> {
    if value["status"] != "completed" {
        return Err(PlannerError);
    }
    let output = value["output"].as_array().ok_or(PlannerError)?;
    let mut texts = output
        .iter()
        .filter(|v| v["type"] == "message")
        .flat_map(|v| v["content"].as_array().into_iter().flatten())
        .filter(|v| v["type"] == "output_text")
        .filter_map(|v| v["text"].as_str());
    let text = texts.next().ok_or(PlannerError)?;
    if texts.next().is_some() {
        return Err(PlannerError);
    }
    let decision: Decision = serde_json::from_str(text).map_err(|_| PlannerError)?;
    // Installation evidence must come from an actual executor, never a model assertion.
    if decision.command.as_ref().is_some_and(|c| {
        matches!(
            c.action,
            orchestrator_protocol::collaboration::GoalAction::Evaluate { .. }
        )
    }) {
        return Err(PlannerError);
    }
    Ok(decision.command)
}

#[cfg(test)]
#[path = "../tests/internal/planner.rs"]
mod tests;
