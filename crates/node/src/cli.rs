//! Local JSON-lines control and outbound coordinator synchronization.
//! Execution is opt-in through local stdio or authenticated outbound delivery.

use std::{path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use orchestrator_agents::fake::FakeAgent;
use orchestrator_protocol::{
    agent::AgentCommand,
    ids::{CommandId, GroupId, NodeId, SessionId},
};
use serde::{Deserialize, Serialize};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

use crate::{
    NodeError,
    journal::Journal,
    model::{CommandRecord, NodeIdentity, PendingEvent, SessionRecord, WorkspaceMount},
    process::{Launcher, LocalLauncher},
    runtime::NodeRuntime,
    worker,
};

/// Maximum local input frame, bounded before JSON decoding.
pub const MAX_FRAME_BYTES: usize = 1_048_576;

/// Command-line entry point for the local fake-specialist runtime.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Local node runtime with durable fake-specialist processes"
)]
pub struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Debug, Subcommand)]
enum Mode {
    /// Inspect runtime permissions and authenticate human decisions locally.
    Permissions(crate::review_cli::ReviewCli),
    /// Synchronize this node's journal with a separate routing service.
    Sync {
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        coordinator: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        profile_file: PathBuf,
        /// Upload one batch and exit instead of watching continuously.
        #[arg(long)]
        once: bool,
        /// Own the supervisor and accept work from the authenticated router.
        #[arg(long)]
        execute: bool,
        /// JSON file selecting a goal and its planning model. Requires execution mode.
        #[arg(long, requires = "execute")]
        super_agent: Option<PathBuf>,
    },
    /// Run the local JSON-lines supervisor; EOF leaves specialists running.
    Run {
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        group: GroupId,
        #[arg(long)]
        node: NodeId,
    },
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        session: SessionId,
        #[arg(long)]
        instance: String,
    },
    #[command(hide = true)]
    Tools {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        parent: CommandId,
        #[arg(long)]
        instance: String,
    },
    /// Request cooperative shutdown of all local specialist processes.
    Stop {
        #[arg(long)]
        state_dir: PathBuf,
    },
}

/// Local control requests; not a public network API.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Register an operator-approved local directory.
    Mount {
        /// Directory registration.
        mount: WorkspaceMount,
    },
    /// Durably submit a specialist command.
    Submit {
        /// Versioned command.
        command: Box<AgentCommand>,
    },
    /// Read the original worker identity.
    Session {
        /// Session to inspect.
        session_id: SessionId,
    },
    /// Read a command without dispatching it again.
    Command {
        /// Idempotency identity.
        command_id: CommandId,
    },
    /// Read undelivered results.
    Events,
    /// Acknowledge a single result.
    Acknowledge {
        /// Local event sequence.
        sequence: i64,
    },
    /// Stop this supervisor, keeping specialists alive.
    Shutdown,
}

/// One complete response per request, plus a startup readiness response.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Supervisor owns the lock and is ready.
    Ready {
        /// Bound node identity.
        identity: NodeIdentity,
    },
    /// Workspace registration succeeded.
    Mounted,
    /// A command receipt, never an implicit promise of completion.
    Receipt {
        /// Persisted command.
        record: Box<CommandRecord>,
    },
    /// Session inspection response.
    Session {
        /// None when unknown.
        record: Option<Box<SessionRecord>>,
    },
    /// Command inspection response.
    Command {
        /// None when unknown.
        record: Option<Box<CommandRecord>>,
    },
    /// Ordered pending results.
    Events {
        /// Up to 100 events.
        events: Vec<PendingEvent>,
    },
    /// Acknowledgment result.
    Acknowledged {
        /// Whether the event exists, including an already acknowledged event.
        found: bool,
    },
    /// Supervisor is exiting without stopping specialists.
    Bye,
    /// Explicit request rejection.
    Error {
        /// Human-readable diagnostic.
        message: String,
    },
}

/// Execute a parsed command line.
///
/// # Errors
/// Reports startup, process, stream, and storage errors.
pub async fn run(cli: Cli, executable: fn() -> std::io::Result<PathBuf>) -> Result<(), NodeError> {
    run_with_io(
        cli,
        executable,
        &mut BufReader::new(tokio::io::stdin()),
        &mut tokio::io::stdout(),
    )
    .await
}

async fn run_with_io(
    cli: Cli,
    executable: fn() -> std::io::Result<PathBuf>,
    input: &mut (dyn AsyncBufRead + Unpin + Send),
    mut output: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<(), NodeError> {
    match cli.mode {
        Mode::Permissions(command) => write_response(&mut output, &command.run().await?).await,
        Mode::Sync {
            state_dir,
            coordinator,
            token_file,
            profile_file,
            once,
            execute,
            super_agent,
        } => {
            let token = std::fs::read_to_string(token_file)?;
            let token = orchestrator_protocol::sync::NodeToken::try_from(token.trim().to_owned())
                .map_err(|_| NodeError::Rejected("invalid node token file"))?;
            let profile = serde_json::from_slice(&std::fs::read(profile_file)?)?;
            let remote = crate::sync::Remote::new(&coordinator, &token)?;
            let journal = Journal::attach(&state_dir.join("node.sqlite")).await?;
            if execute {
                let identity = journal.identity();
                journal.close().await;
                let mut runtime =
                    NodeRuntime::open(&state_dir, identity, LocalLauncher::new(executable()?))
                        .await?;
                if let Some(path) = super_agent {
                    return run_super_agent(&remote, &mut runtime, &profile, &path, once).await;
                }
                return if once {
                    remote.cycle(&mut runtime, &profile).await
                } else {
                    remote
                        .execute(&mut runtime, &profile, tokio::signal::ctrl_c())
                        .await
                };
            }
            if once {
                remote.synchronize(&journal, &profile).await.map(|_| ())
            } else {
                remote
                    .follow(&journal, &profile, tokio::signal::ctrl_c())
                    .await
            }
        }
        Mode::Run {
            state_dir,
            group,
            node,
        } => {
            let launcher = LocalLauncher::new(executable()?);
            let mut runtime = NodeRuntime::open(
                &state_dir,
                NodeIdentity {
                    group_id: group,
                    node_id: node,
                },
                launcher,
            )
            .await?;
            serve(&mut runtime, input, output).await
        }
        Mode::Worker {
            journal,
            session,
            instance,
        } => run_worker(&journal, session, &instance, executable).await,
        Mode::Tools {
            journal,
            parent,
            instance,
        } => {
            let journal = Journal::attach(&journal).await?;
            crate::tool_server::serve(&journal, parent, &instance, input, output).await
        }
        Mode::Stop { state_dir } => {
            Journal::attach(&state_dir.join("node.sqlite"))
                .await?
                .request_stop_all()
                .await
        }
    }
}

/// Serve bounded local requests while continuously reaping and monitoring workers.
///
/// # Errors
/// Returns on broken streams, oversized frames, or supervision failures.
pub async fn serve<L: Launcher, R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    runtime: &mut NodeRuntime<L>,
    mut input: R,
    mut output: W,
) -> Result<(), NodeError> {
    write_response(
        &mut output,
        &Response::Ready {
            identity: runtime.journal().identity(),
        },
    )
    .await?;
    let mut buffer = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        // read_until is cancellation-safe and appends into this persistent buffer.
        // Recompute the remaining allowance after every supervision tick.
        let mut limited = (&mut input).take((MAX_FRAME_BYTES + 1 - buffer.len()) as u64);
        tokio::select! {
            read = limited.read_until(b'\n', &mut buffer) => {
                let bytes = read?;
                if buffer.len() > MAX_FRAME_BYTES { return Err(NodeError::Rejected("input frame exceeds 1 MiB")); }
                if bytes == 0 && buffer.is_empty() { return Ok(()); }
                let result = match serde_json::from_slice::<Request>(&buffer) {
                    Ok(request) => dispatch(runtime, request).await,
                    Err(error) => Err(error.into()),
                };
                buffer.clear();
                let response = result.unwrap_or_else(|error| Response::Error { message: error.to_string() });
                write_response(&mut output, &response).await?;
                if matches!(response, Response::Bye) { return Ok(()); }
            }
            _ = interval.tick() => runtime.tick().await?,
        }
    }
}

async fn dispatch<L: Launcher>(
    runtime: &mut NodeRuntime<L>,
    request: Request,
) -> Result<Response, NodeError> {
    Ok(match request {
        Request::Mount { mount } => {
            runtime
                .journal()
                .mount(mount.workspace_id, &mount.path)
                .await?;
            Response::Mounted
        }
        Request::Submit { command } => Response::Receipt {
            record: Box::new(runtime.submit(&command).await?),
        },
        Request::Session { session_id } => Response::Session {
            record: runtime.journal().session(session_id).await?.map(Box::new),
        },
        Request::Command { command_id } => Response::Command {
            record: runtime.journal().command(command_id).await?.map(Box::new),
        },
        Request::Events => Response::Events {
            events: runtime.journal().pending_events().await?,
        },
        Request::Acknowledge { sequence } => Response::Acknowledged {
            found: runtime.journal().acknowledge(sequence).await?,
        },
        Request::Shutdown => Response::Bye,
    })
}

async fn write_response<W: AsyncWrite + Unpin, T: Serialize>(
    output: &mut W,
    response: &T,
) -> Result<(), NodeError> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SuperAgentSettings {
    goal_id: orchestrator_protocol::ids::GoalId,
    endpoint: String,
    model: String,
    key_file: PathBuf,
    turn_limit: u32,
    #[serde(default)]
    prompt_dir: Option<PathBuf>,
}
async fn run_super_agent(
    remote: &crate::sync::Remote,
    runtime: &mut NodeRuntime<LocalLauncher>,
    profile: &orchestrator_protocol::sync::NodeProfile,
    path: &std::path::Path,
    once: bool,
) -> Result<(), NodeError> {
    let settings: SuperAgentSettings = serde_json::from_slice(&std::fs::read(path)?)?;
    let key = std::fs::read_to_string(&settings.key_file)?;
    let mut planner = orchestrator_agents::planner::ModelPlanner::new(
        &settings.endpoint,
        key.trim(),
        &settings.model,
    )?;
    if let Some(directory) = settings.prompt_dir {
        planner = planner.with_prompt(orchestrator_prompts::Prompt::load(
            &directory,
            orchestrator_prompts::Kind::Planner,
        )?);
    }
    if once {
        remote
            .super_step(
                runtime,
                profile,
                settings.goal_id,
                &planner,
                settings.turn_limit,
            )
            .await
    } else {
        remote
            .supervise(
                runtime,
                profile,
                settings.goal_id,
                &planner,
                settings.turn_limit,
                tokio::signal::ctrl_c(),
            )
            .await
    }
}

#[cfg(test)]
#[path = "../tests/internal/cli.rs"]
mod tests;

async fn run_worker(
    journal: &std::path::Path,
    session: SessionId,
    instance: &str,
    executable: fn() -> std::io::Result<PathBuf>,
) -> Result<(), NodeError> {
    let journal = Journal::attach(journal).await?;
    let identity = journal.identity();
    if let Some(config) = journal.execution_config(session).await? {
        worker::run(
            &journal,
            session,
            instance,
            crate::local_provider::LocalAgent::new(
                journal.clone(),
                instance.to_owned(),
                config,
                executable()?,
            ),
        )
        .await
    } else {
        worker::run(
            &journal,
            session,
            instance,
            FakeAgent::new(identity.group_id, identity.node_id),
        )
        .await
    }
}
