//! Operator CLI. Provisioning requires database access; node APIs do not enroll.

use clap::{Parser, Subcommand};
use orchestrator_coordinator::{
    selector::{LlmSelector, Selector, Unconfigured},
    store::Store,
    web,
};
use orchestrator_protocol::{
    ids::{GoalId, GroupId, NodeId, ReleaseId},
    sync::NodeToken,
};
use std::{io, net::SocketAddr, path::PathBuf};

#[derive(Parser)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Publish a capability manifest with Markdown recipe, instructions and evaluation files.
    Catalog {
        #[arg(long)]
        group: GroupId,
        #[arg(long)]
        file: PathBuf,
    },
    /// Review an exact learning request; decision JSON uses approve/activate/reject.
    Learning {
        #[arg(long)]
        group: GroupId,
        #[arg(long)]
        goal: GoalId,
        #[arg(long)]
        node: NodeId,
        #[arg(long)]
        release: ReleaseId,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        decision_file: PathBuf,
    },
    /// Serve behind a TLS reverse proxy; defaults to loopback only.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        #[arg(long, requires = "model_key_file")]
        model: Option<String>,
        #[arg(long, requires = "model")]
        model_key_file: Option<PathBuf>,
        #[arg(long, default_value = "https://api.openai.com/v1/responses")]
        model_endpoint: String,
        /// Read router/selection.md from this directory at startup.
        #[arg(long, requires = "model")]
        prompt_dir: Option<PathBuf>,
    },
    /// Enroll an identity using a securely generated 64-hex-character token file.
    Enroll {
        #[arg(long)]
        group: GroupId,
        #[arg(long)]
        node: NodeId,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Disable future connections without deleting the node's event history.
    Revoke {
        #[arg(long)]
        group: GroupId,
        #[arg(long)]
        node: NodeId,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let store = Store::open(&cli.database_url)
        .await
        .map_err(io::Error::other)?;
    match cli.mode {
        Mode::Catalog { group, file } => {
            let release = orchestrator_coordinator::catalog_source::read(&file)?;
            store
                .publish_release(group, &release)
                .await
                .map_err(io::Error::other)
        }
        Mode::Learning {
            group,
            goal,
            node,
            release,
            revision,
            decision_file,
        } => {
            let decision = serde_json::from_slice(&std::fs::read(decision_file)?)?;
            let view = store
                .decide_learning(group, goal, revision, node, release, &decision)
                .await
                .map_err(io::Error::other)?;
            println!("{}", serde_json::json!(view));
            Ok(())
        }
        Mode::Serve {
            listen,
            model,
            model_key_file,
            model_endpoint,
            prompt_dir,
        } => {
            let selector: std::sync::Arc<dyn Selector> = match (model, model_key_file) {
                (Some(model), Some(path)) => {
                    let mut model = LlmSelector::new(
                        &model_endpoint,
                        std::fs::read_to_string(path)?.trim(),
                        &model,
                    )
                    .map_err(io::Error::other)?;
                    if let Some(directory) = prompt_dir {
                        model = model.with_prompt(orchestrator_prompts::Prompt::load(
                            &directory,
                            orchestrator_prompts::Kind::Router,
                        )?);
                    }
                    std::sync::Arc::new(model)
                }
                _ => std::sync::Arc::new(Unconfigured),
            };
            let listener = tokio::net::TcpListener::bind(listen).await?;
            axum::serve(listener, web::router_with_selector(store, selector))
                .with_graceful_shutdown(async {
                    // Failure to install a signal listener also initiates shutdown.
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
        }
        Mode::Enroll {
            group,
            node,
            token_file,
        } => {
            let contents = std::fs::read_to_string(token_file)?;
            let token =
                NodeToken::try_from(contents.trim().to_owned()).map_err(io::Error::other)?;
            store
                .enroll(group, node, &token)
                .await
                .map_err(io::Error::other)
        }
        Mode::Revoke { group, node } => store
            .revoke(group, node)
            .await
            .map(|_| ())
            .map_err(io::Error::other),
    }
}
