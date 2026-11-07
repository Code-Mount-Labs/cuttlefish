//! Authenticated chat facade. The router persists and dispatches; nodes execute.
use crate::{Error, state::Machine};
use orchestrator_coordinator::{routing::RoutingService, store::Store};
use orchestrator_node::{
    execution::{ExecutionConfig, executable},
    journal::Journal,
    model::NodeIdentity,
};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    ids::{NodeId, ThreadId},
    routing::{DispatchView, RouteAction, RouteRequest},
    sync::NodeToken,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};

impl Machine {
    /// Open only this installation's node journal for local workspace configuration.
    ///
    /// # Errors
    /// Rejects router-only installations or unavailable storage.
    pub async fn agent_journal(&self) -> Result<Journal, Error> {
        let identity = self.identity().await?;
        if !identity.registration.role.has_agent() {
            return Err(Error::Invalid("configure_workspaces_on_an_agent"));
        }
        let directory = self.directory().join("agent");
        std::fs::create_dir_all(&directory)?;
        Journal::open(
            &directory.join("node.sqlite"),
            NodeIdentity {
                group_id: identity.registration.group_id,
                node_id: NodeId::from(identity.registration.installation_id),
            },
        )
        .await
        .map_err(|_| Error::Dependency("agent_journal_unavailable"))
    }
    /// Create an isolated workspace and pin its provider and edit permission.
    ///
    /// # Errors
    /// Rejects missing providers, changed configuration and storage failure.
    pub async fn create_workspace(&self, workspace: ProviderWorkspace) -> Result<(), Error> {
        self.create_workspace_with(workspace, self.provider_path)
            .await
    }
    async fn create_workspace_with(
        &self,
        workspace: ProviderWorkspace,
        resolve: fn(LocalProvider) -> Option<PathBuf>,
    ) -> Result<(), Error> {
        let executable =
            resolve(workspace.provider).ok_or(Error::Invalid("provider_cli_not_installed"))?;
        let directory = self
            .directory()
            .join("workspaces")
            .join(workspace.workspace_id.to_string());
        std::fs::create_dir_all(&directory)?;
        let config = execution_config(
            workspace,
            executable,
            &directory,
            &self.directory().join("prompts/execution/specialist.md"),
        )?;
        self.agent_journal()
            .await?
            .configure_execution(&config)
            .await
            .map_err(|_| Error::Invalid("workspace_configuration_rejected"))
    }
    /// List workspaces on this machine, including their operator-visible directory.
    ///
    /// # Errors
    /// Reports missing node configuration or local storage failures.
    pub async fn local_workspaces(&self) -> Result<Vec<ExecutionConfig>, Error> {
        self.agent_journal()
            .await?
            .executions()
            .await
            .map_err(|_| Error::Dependency("agent_journal_unavailable"))
    }
    async fn chat_store(&self) -> Result<(Store, NodeToken), Error> {
        let own = self.identity().await?.registration;
        let store = self.execution_store().await?;
        let token = own.credential;
        if store.identity(&token).await.is_err() {
            let _ = store
                .enroll(own.group_id, NodeId::from(own.installation_id), &token)
                .await;
        }
        if store
            .identity(&token)
            .await
            .map_err(|_| Error::Unauthorized)?
            != (own.group_id, NodeId::from(own.installation_id))
        {
            return Err(Error::Unauthorized);
        }
        Ok((store, token))
    }
    /// Discover group workspaces for the chat composer.
    ///
    /// # Errors
    /// Reports unfinished database setup, authentication or storage failures.
    pub async fn chat_workspaces(&self) -> Result<Vec<Value>, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .provider_workspaces(&token)
            .await
            .map_err(|_| Error::Dependency("workspace_directory_unavailable"))
    }
    /// List recent durable conversations in this group.
    ///
    /// # Errors
    /// Reports unfinished setup, authentication or storage failures.
    pub async fn chats(&self) -> Result<Vec<Value>, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .conversations(&token)
            .await
            .map_err(|_| Error::Dependency("chat_history_unavailable"))
    }
    /// Read persisted user messages, execution state and provider replies.
    ///
    /// # Errors
    /// Reports unfinished setup, authentication or storage failures.
    pub async fn chat(&self, thread: ThreadId) -> Result<Vec<DispatchView>, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .conversation(&token, thread)
            .await
            .map_err(|_| Error::Dependency("chat_history_unavailable"))
    }
    /// Submit an idempotent message through the existing affinity-aware router.
    ///
    /// # Errors
    /// Rejects unconfigured workspaces, busy conversations and unavailable routing.
    pub async fn send_chat(&self, request: RouteRequest) -> Result<DispatchView, Error> {
        let (store, token) = self.chat_store().await?;
        if let RouteAction::Start {
            workspace_id,
            capabilities,
            ..
        } = &request.action
        {
            let rows = store
                .provider_workspaces(&token)
                .await
                .map_err(|_| Error::Dependency("workspace_directory_unavailable"))?;
            let found = rows.iter().any(|row| {
                serde_json::from_value::<ProviderWorkspace>(row["workspace"].clone()).is_ok_and(
                    |workspace| {
                        workspace.workspace_id == *workspace_id
                            && capabilities.len() == 1
                            && capabilities
                                .iter()
                                .any(|item| item.as_str() == workspace.provider.name())
                    },
                )
            });
            if !found {
                return Err(Error::Invalid("select_a_registered_provider_workspace"));
            }
        }
        RoutingService::new(store, Arc::new(self.clone()))
            .submit(&token, request)
            .await
            .map_err(|_| Error::Dependency("message_not_accepted_check_agent_and_pending_turn"))
    }
}
fn execution_config(
    workspace: ProviderWorkspace,
    executable: PathBuf,
    directory: &std::path::Path,
    instructions: &std::path::Path,
) -> Result<ExecutionConfig, Error> {
    Ok(ExecutionConfig {
        workspace,
        executable,
        directory: directory.canonicalize()?,
        instructions: instructions.canonicalize()?,
    })
}
pub(crate) fn provider_path(provider: LocalProvider) -> Option<PathBuf> {
    provider_path_in(provider, std::env::var_os("PATH"), std::env::var_os("HOME"))
}
fn provider_path_in(
    provider: LocalProvider,
    search: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let mut paths: Vec<_> = std::env::split_paths(&search.unwrap_or_default()).collect();
    if let Some(home) = home {
        paths.push(PathBuf::from(home).join(".local/bin"));
    }
    executable(provider.name(), &std::env::join_paths(paths).ok()?)
}
/// Check sign-in via the provider CLI; never read or return credential files.
impl Machine {
    /// Check local subscription sign-ins without reading credential files.
    pub async fn providers(&self) -> Vec<Value> {
        providers_with(&self.provider_path, Duration::from_secs(5)).await
    }
}
async fn providers_with(
    resolve: &(dyn Fn(LocalProvider) -> Option<PathBuf> + Sync),
    timeout: Duration,
) -> Vec<Value> {
    let mut statuses = Vec::new();
    for provider in [LocalProvider::Codex, LocalProvider::Claude] {
        let path = resolve(provider);
        let signed_in = if let Some(path) = &path {
            let mut command = tokio::process::Command::new(path);
            command.kill_on_drop(true).args(match provider {
                LocalProvider::Codex => vec!["login", "status"],
                LocalProvider::Claude => vec!["auth", "status"],
            });
            match tokio::time::timeout(timeout, command.output()).await {
                Ok(Ok(output)) => {
                    output.status.success()
                        && (provider == LocalProvider::Codex
                            || serde_json::from_slice::<Value>(&output.stdout)
                                .is_ok_and(|value| value["loggedIn"] == true))
                }
                _ => false,
            }
        } else {
            false
        };
        statuses
            .push(json!({"provider":provider,"installed":path.is_some(),"signed_in":signed_in}));
    }
    statuses
}

#[cfg(all(test, unix))]
#[path = "../tests/internal/chat.rs"]
mod tests;

impl Machine {
    /// Store a bounded file chunk in the group router's execution database.
    ///
    /// # Errors
    /// Rejects invalid uploads, unfinished database setup and storage failures.
    pub async fn upload_attachment(
        &self,
        chunk: &orchestrator_protocol::attachments::AttachmentChunk,
    ) -> Result<bool, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .put_attachment(&token, chunk)
            .await
            .map_err(|_| Error::Invalid("attachment_upload_rejected"))
    }
    /// Download a verified chunk without exposing a node filesystem path.
    ///
    /// # Errors
    /// Rejects unknown, incomplete or inaccessible files and storage failures.
    pub async fn attachment_chunk(
        &self,
        id: orchestrator_protocol::ids::AttachmentId,
        index: u32,
    ) -> Result<orchestrator_protocol::attachments::AttachmentChunk, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .attachment_chunk(&token, id, index)
            .await
            .map_err(|_| Error::Invalid("attachment_unavailable"))
    }
}

impl Machine {
    /// Read factual provider progress for this group's conversation.
    ///
    /// # Errors
    /// Reports authentication, setup or storage failures.
    pub async fn chat_activity(&self, thread: ThreadId) -> Result<Value, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .conversation_activity(&token, thread)
            .await
            .map_err(|_| Error::Dependency("activity_unavailable"))
    }
}
