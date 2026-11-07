//! Constrained model selection. Model output never grants execution authority.
use crate::Error;
use orchestrator_protocol::{ids::NodeId, routing::RoutingContext};
use reqwest::{Client, ClientBuilder, Url, header::HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fmt::Debug, future::Future, pin::Pin, time::Duration};

/// Object-safe asynchronous selection boundary, replaceable in deterministic tests.
pub trait Selector: Debug + Send + Sync {
    /// Choose a candidate, or fail without creating any execution.
    fn choose<'a>(
        &'a self,
        context: &'a RoutingContext,
    ) -> Pin<Box<dyn Future<Output = Result<NodeId, Error>> + Send + 'a>>;
}

/// Default when model credentials are not configured; affinity still works.
#[derive(Debug)]
pub struct Unconfigured;
impl Selector for Unconfigured {
    fn choose<'a>(
        &'a self,
        _: &'a RoutingContext,
    ) -> Pin<Box<dyn Future<Output = Result<NodeId, Error>> + Send + 'a>> {
        Box::pin(async { Err(Error::Model) })
    }
}

/// `OpenAI` Responses API adapter with structured output and no tools.
#[derive(Debug)]
pub struct LlmSelector {
    client: Client,
    endpoint: Url,
    authorization: HeaderValue,
    model: String,
    instructions: orchestrator_prompts::Prompt,
}

impl LlmSelector {
    /// Configure an explicit model and endpoint. Keys are redacted from Debug.
    ///
    /// # Errors
    /// Rejects invalid credentials, insecure non-loopback URLs and client errors.
    pub fn new(endpoint: &str, key: &str, model: &str) -> Result<Self, Error> {
        Self::with_builder(endpoint, key, model, Client::builder())
    }

    fn with_builder(
        endpoint: &str,
        key: &str,
        model: &str,
        builder: ClientBuilder,
    ) -> Result<Self, Error> {
        let endpoint = Url::parse(endpoint).map_err(|_| Error::Model)?;
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
            return Err(Error::Model);
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| Error::Model)?;
        authorization.set_sensitive(true);
        let client = builder
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Model)?;
        Ok(Self {
            client,
            endpoint,
            authorization,
            model: model.to_owned(),
            instructions: orchestrator_prompts::Prompt::bundled(orchestrator_prompts::Kind::Router),
        })
    }

    /// Replace the bundled wording with a validated, operator-reviewed prompt.
    #[must_use]
    pub fn with_prompt(mut self, prompt: orchestrator_prompts::Prompt) -> Self {
        self.instructions = prompt;
        self
    }

    async fn select(&self, context: &RoutingContext) -> Result<NodeId, Error> {
        let choices: Vec<String> = context
            .candidates
            .iter()
            .map(|c| c.node_id.to_string())
            .collect();
        if choices.is_empty() {
            return Err(Error::NoCapacity);
        }
        let body = json!({"model":self.model,"store":false,"max_output_tokens":512,
            "instructions":self.instructions.as_str(),
            "input":[{"role":"user","content":json!(context).to_string()}],
            "text":{"format":{"type":"json_schema","name":"route_selection","strict":true,"schema":{"type":"object","properties":{"node_id":{"type":"string","enum":choices}},"required":["node_id"],"additionalProperties":false}}}});
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header("authorization", self.authorization.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| Error::Model)?;
        if !response.status().is_success() {
            return Err(Error::Model);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Model)? {
            if bytes.len() + chunk.len() > 262_144 {
                return Err(Error::Model);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Model)?;
        let chosen = parse_response(&value)?;
        if !context.candidates.iter().any(|c| c.node_id == chosen) {
            return Err(Error::Model);
        }
        Ok(chosen)
    }
}
impl Selector for LlmSelector {
    fn choose<'a>(
        &'a self,
        context: &'a RoutingContext,
    ) -> Pin<Box<dyn Future<Output = Result<NodeId, Error>> + Send + 'a>> {
        Box::pin(self.select(context))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    node_id: NodeId,
}

fn parse_response(value: &Value) -> Result<NodeId, Error> {
    if value["status"] != "completed" {
        return Err(Error::Model);
    }
    let output = value["output"].as_array().ok_or(Error::Model)?;
    let mut texts = output
        .iter()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|item| item["type"] == "output_text")
        .filter_map(|item| item["text"].as_str());
    let text = texts.next().ok_or(Error::Model)?;
    if texts.next().is_some() {
        return Err(Error::Model);
    }
    let choice: Choice = serde_json::from_str(text).map_err(|_| Error::Model)?;
    Ok(choice.node_id)
}

#[cfg(test)]
#[path = "../tests/internal/selector.rs"]
mod tests;
