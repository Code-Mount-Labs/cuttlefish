//! Local role, model, and execution configuration. Secrets never go to the registry.
use crate::{Error, resources::DatabaseSpec, state::Machine};
use orchestrator_coordinator::{
    selector::{LlmSelector, Selector},
    store::Store,
};
use orchestrator_protocol::{ids::NodeId, installation::Registration, routing::RoutingContext};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

/// Private installation identity and endpoints persisted before network use.
#[derive(Debug)]
pub struct Identity {
    /// Original registration, including redacted credentials.
    pub registration: Registration,
    /// Hosted account/group registry origin.
    pub platform_origin: String,
    /// Local dashboard's externally visible origin.
    pub public_origin: String,
}
/// Model settings accepted only by the authenticated local dashboard.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSettings {
    /// Explicit Responses-compatible endpoint.
    pub endpoint: String,
    /// Provider model identifier.
    pub model: String,
    /// Private API key, never returned by settings reads.
    pub api_key: String,
}
impl std::fmt::Debug for ModelSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ModelSettings([redacted])")
    }
}
impl Machine {
    /// Read the immutable local identity.
    ///
    /// # Errors
    /// Reports missing setup or invalid local storage.
    pub async fn identity(&self) -> Result<Identity, Error> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT registration,platform_origin,public_origin FROM installation WHERE singleton=1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let (registration, platform_origin, public_origin) =
            row.ok_or(Error::Invalid("installation_not_initialized"))?;
        Ok(Identity {
            registration: serde_json::from_str(&registration)?,
            platform_origin,
            public_origin,
        })
    }
    /// Persist a local setting. Callers must authenticate at their boundary.
    ///
    /// # Errors
    /// Reports local storage failure.
    pub(crate) async fn save_setting(&self, name: &str, value: &str) -> Result<(), Error> {
        sqlx::query("INSERT INTO local_settings(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value").bind(name).bind(value).execute(&self.pool).await?;
        Ok(())
    }
    pub(crate) async fn setting(&self, name: &str) -> Result<Option<String>, Error> {
        Ok(
            sqlx::query_scalar("SELECT value FROM local_settings WHERE name=?1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    /// Save a validated routing model locally; the API key is never published.
    ///
    /// # Errors
    /// Rejects invalid provider configuration and local storage errors.
    pub async fn configure_model(&self, input: &ModelSettings) -> Result<(), Error> {
        LlmSelector::new(&input.endpoint, &input.api_key, &input.model)
            .map_err(|_| Error::Invalid("invalid_model_configuration"))?;
        self.save_setting("model", &serde_json::to_string(input)?)
            .await
    }
    /// Read public model fields without exposing the key.
    ///
    /// # Errors
    /// Reports invalid local storage.
    pub async fn model_status(&self) -> Result<serde_json::Value, Error> {
        let saved = self.setting("model").await?;
        let Some(saved) = saved else {
            return Ok(serde_json::json!({"configured":false}));
        };
        let model: ModelSettings = serde_json::from_str(&saved)?;
        Ok(serde_json::json!({"configured":true,"endpoint":model.endpoint,"model":model.model}))
    }
    /// Open the user-owned execution store only after resource verification.
    /// It is separate from both the local setup journal and platform database.
    ///
    /// # Errors
    /// Rejects agent-only roles, unfinished setup, and unavailable databases.
    pub async fn execution_store(&self) -> Result<Store, Error> {
        if !self.identity().await?.registration.role.has_router() {
            return Err(Error::Invalid("this_installation_has_no_router"));
        }
        let store = self
            .execution
            .get_or_try_init(|| async {
                if self.status().await?.state != "ready" {
                    return Err(Error::Dependency("database_setup_required"));
                }
                let spec: String =
                    sqlx::query_scalar("SELECT spec FROM resources WHERE singleton=1")
                        .fetch_one(&self.pool)
                        .await?;
                let spec: DatabaseSpec = serde_json::from_str(&spec)?;
                Store::open(&spec.connection_url())
                    .await
                    .map_err(|_| Error::Dependency("execution_database_unavailable"))
            })
            .await?;
        Ok(store.clone())
    }
}
impl Selector for Machine {
    fn choose<'a>(
        &'a self,
        context: &'a RoutingContext,
    ) -> Pin<Box<dyn Future<Output = Result<NodeId, orchestrator_coordinator::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let saved = self
                .setting("model")
                .await
                .map_err(|_| orchestrator_coordinator::Error::Model)?
                .ok_or(orchestrator_coordinator::Error::Model)?;
            let model: ModelSettings =
                serde_json::from_str(&saved).map_err(|_| orchestrator_coordinator::Error::Model)?;
            LlmSelector::new(&model.endpoint, &model.api_key, &model.model)?
                .with_prompt(
                    orchestrator_prompts::Prompt::load(
                        &self.directory().join("prompts"),
                        orchestrator_prompts::Kind::Router,
                    )
                    .map_err(|_| orchestrator_coordinator::Error::Model)?,
                )
                .choose(context)
                .await
        })
    }
}
/// Validate a configured HTTP origin before sending installation credentials.
///
/// # Errors
/// Requires HTTPS outside literal loopback and forbids embedded credentials.
pub fn validate_origin(value: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(value).map_err(|_| Error::Invalid("invalid_origin"))?;
    let local = url
        .host_str()
        .and_then(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::Invalid("use_https_or_literal_loopback_origin"));
    }
    Ok(url.origin().ascii_serialization())
}

/// Write missing editable defaults without overwriting an operator's changes.
pub(crate) fn initialize_prompts(directory: &std::path::Path) -> Result<(), Error> {
    use orchestrator_prompts::{Kind, Prompt};
    use std::io::Write;
    for (path, kind) in [
        ("router/selection.md", Kind::Router),
        ("super-agent/planner.md", Kind::Planner),
    ] {
        let target = directory.join("prompts").join(path);
        let parent = target
            .parent()
            .ok_or(Error::Invalid("invalid_prompt_path"))?;
        std::fs::create_dir_all(parent)?;
        if target.try_exists()? {
            continue;
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(Prompt::bundled(kind).as_str().as_bytes())?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&target) {
            Ok(_) => {
                #[cfg(unix)]
                std::fs::File::open(parent)?.sync_all()?;
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
    }
    Ok(())
}
