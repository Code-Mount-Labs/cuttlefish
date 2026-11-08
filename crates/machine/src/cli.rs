//! Human setup flow. Secrets are kept out of command arguments and shell history.
use crate::{
    Error, docker::DockerBackend, registry::Client, settings::validate_origin, state::Machine,
};
use clap::{Parser, Subcommand};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationRole, Registration},
    sync::NodeToken,
};
use std::{
    io::{self, BufRead, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
};
#[cfg(test)]
use uuid::Uuid;

/// Command-line entry point for a local Cuttlefish installation.
#[derive(Debug, Parser)]
#[command(version, about = "Install and run a Cuttlefish router, agent, or both")]
pub struct Cli {
    /// Private local state directory. Defaults to ~/.cuttlefish.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Mode,
}
#[derive(Debug, Subcommand)]
enum Mode {
    /// Join a group, select a role, and open the local setup dashboard.
    Setup {
        /// Keep running in this terminal instead of installing a macOS login service.
        #[arg(long)]
        foreground: bool,
        #[arg(
            long,
            default_value = "https://platform-production-105c.up.railway.app"
        )]
        platform: String,
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
    /// Resume an existing installation, including interrupted database setup.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
    /// Install/start Docker with interactive OS authorization if needed.
    InstallDocker,
    /// Check for updates, install a verified release, and restart the background service.
    Update {
        /// Check availability without installing.
        #[arg(long)]
        check: bool,
    },
    /// Resume a durable update independently of the server.
    #[command(hide = true)]
    UpdateWorker,
    /// Manage background startup for this installation (macOS).
    Service {
        #[command(subcommand)]
        action: crate::service::ServiceAction,
        #[arg(long, global = true, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
}
/// Run setup or an existing local installation.
///
/// # Errors
/// Reports invalid setup input, network, storage and listener errors.
pub async fn run(cli: Cli) -> Result<(), Error> {
    run_with(
        cli,
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
        std::env::current_exe,
        &mut io::stdin().lock(),
        &mut io::stdout(),
    )
    .await
}
async fn run_with(
    cli: Cli,
    home: Option<std::ffi::OsString>,
    executable: fn() -> io::Result<PathBuf>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<(), Error> {
    run_with_environment(
        cli,
        home,
        executable,
        input,
        output,
        SetupEnvironment {
            os: std::env::consts::OS,
            script: include_str!("../../../install/service-macos.sh"),
            browser: "open",
        },
    )
    .await
}
struct SetupEnvironment<'a> {
    os: &'a str,
    script: &'a str,
    browser: &'a str,
}
async fn run_with_environment(
    cli: Cli,
    home: Option<std::ffi::OsString>,
    executable: fn() -> io::Result<PathBuf>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    environment: SetupEnvironment<'_>,
) -> Result<(), Error> {
    let (platform, listen, foreground) = match cli.command {
        Mode::InstallDocker => return crate::dependencies::install_docker(true).await,
        Mode::Update { .. } | Mode::UpdateWorker => {
            let directory = cli
                .state_dir
                .or_else(|| {
                    home.as_ref()
                        .map(|home| PathBuf::from(home).join(".cuttlefish"))
                })
                .ok_or(Error::Invalid("provide --state-dir"))?;
            let manager =
                crate::updates::Manager::new(&directory, executable()?, home.map(PathBuf::from));
            return match cli.command {
                Mode::Update { check } => crate::updates::run_cli(manager, check).await,
                _ => crate::updates::run_worker(manager).await,
            };
        }
        Mode::Service { action, listen } => {
            let directory = cli
                .state_dir
                .or_else(|| {
                    home.as_ref()
                        .map(|home| PathBuf::from(home).join(".cuttlefish"))
                })
                .ok_or(Error::Invalid("provide --state-dir"))?;
            return crate::service::manage(
                action,
                &directory,
                &executable()?,
                listen,
                home.as_deref(),
            )
            .await;
        }
        Mode::Setup {
            platform,
            listen,
            foreground,
        } => (Some(platform), listen, foreground),
        Mode::Serve { listen } => (None, listen, true),
    };
    let directory = match cli.state_dir {
        Some(path) => path,
        None => PathBuf::from(home.as_ref().ok_or(Error::Invalid("provide --state-dir"))?)
            .join(".cuttlefish"),
    };
    let machine = Machine::open(&directory, Arc::new(DockerBackend::new(&directory))).await?;
    let setup_mode = platform.is_some();
    if let Some(platform) = platform
        && matches!(
            machine.identity().await,
            Err(Error::Invalid("installation_not_initialized"))
        )
    {
        setup(&machine, &platform, listen, input, output).await?;
    }
    print_dashboard(&machine, output).await?;
    if setup_mode && !foreground && environment.os == "macos" {
        machine.close().await;
        drop(machine);
        crate::service::apply(
            crate::service::ServiceAction::Install,
            &directory,
            &crate::installation::stable(&executable()?),
            listen,
            home.as_deref(),
            environment.os,
            environment.script,
        )
        .await?;
        // Opening a browser is optional; the printed address always remains usable.
        let _ = tokio::process::Command::new(environment.browser)
            .arg(format!("http://{listen}"))
            .status()
            .await;
        writeln!(
            output,
            "Setup is running in your browser. You can close this terminal."
        )?;
        return Ok(());
    }
    let worker =
        executable()?.with_file_name(format!("orchestrator-node{}", std::env::consts::EXE_SUFFIX));
    crate::runtime::serve(machine, listen, worker).await
}
async fn print_dashboard(machine: &Machine, output: &mut dyn Write) -> Result<(), Error> {
    let identity = machine.identity().await?;
    writeln!(output, "Cuttlefish dashboard: {}", identity.public_origin)?;
    if !machine.initialized().await? {
        writeln!(
            output,
            "One-time local setup code: {}",
            machine.bootstrap_token()?.expose()
        )?;
    }
    Ok(())
}

async fn setup(
    machine: &Machine,
    platform: &str,
    listen: SocketAddr,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<(), Error> {
    let client = Client::new(platform)?;
    let platform = client.origin.clone();
    let group: GroupId = ask(input, output, "Group ID", None)?
        .parse()
        .map_err(|_| Error::Invalid("invalid_group_id"))?;
    let invitation = NodeToken::try_from(ask(input, output, "One-time enrollment code", None)?)
        .map_err(|_| Error::Invalid("invalid_enrollment_code"))?;
    let topology = client.preflight(group, &invitation).await?;
    if topology.roles.is_empty() {
        return Err(Error::Invalid(
            "this_group_already_has_a_router_that_is_not_ready",
        ));
    }
    for (index, role) in topology.roles.iter().enumerate() {
        writeln!(output, "{}. {}", index + 1, role_name(*role))?;
    }
    let choice: usize = ask(input, output, "Installation role", Some("1"))?
        .parse()
        .map_err(|_| Error::Invalid("invalid_role_choice"))?;
    let role = *choice
        .checked_sub(1)
        .and_then(|index| topology.roles.get(index))
        .ok_or(Error::Invalid("invalid_role_choice"))?;
    let name = machine_name(ask(input, output, "Machine name", Some("My machine"))?)?;
    let default_origin = format!("http://127.0.0.1:{}", listen.port());
    let answer = ask(
        input,
        output,
        "Remote access: local, managed, or existing HTTPS address",
        Some("local"),
    )?;
    let choice = match answer.as_str() {
        "local" => orchestrator_protocol::remote::RemoteChoice::Local {},
        "managed" => orchestrator_protocol::remote::RemoteChoice::Managed {},
        value => orchestrator_protocol::remote::RemoteChoice::Existing {
            origin: validate_origin(value)?,
        },
    };
    let public_origin = default_origin;
    let registration = Registration {
        group_id: group,
        installation_id: InstallationId::random(),
        role,
        name,
        router_url: role.has_router().then(|| public_origin.clone()),
        credential: NodeToken::random(),
        invitation,
    };
    machine
        .initialize(&registration, &platform, &public_origin)
        .await?;
    machine.set_local_endpoint(&public_origin).await?;
    if !matches!(
        choice,
        orchestrator_protocol::remote::RemoteChoice::Local {}
    ) {
        machine.select_remote(choice).await?;
    }
    if machine.register().await.is_err() {
        writeln!(
            output,
            "Registration will retry in the background using the saved identity."
        )?;
    }
    writeln!(
        output,
        "Open the dashboard to create an administrator and choose existing PostgreSQL, local PostgreSQL, or setup later."
    )?;
    Ok(())
}
fn ask(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    prompt: &str,
    default: Option<&str>,
) -> Result<String, Error> {
    write!(output, "{prompt}")?;
    if let Some(default) = default {
        write!(output, " [{default}]")?;
    }
    write!(output, ": ")?;
    output.flush()?;
    let mut value = String::new();
    if input.take(4097).read_line(&mut value)? == 0 {
        return Err(Error::Invalid("setup_input_closed"));
    }
    if value.len() > 4096 {
        return Err(Error::Invalid("setup_value_too_long"));
    }
    let value = value.trim();
    if value.is_empty() {
        return default
            .map(str::to_owned)
            .ok_or(Error::Invalid("setup_value_required"));
    }
    Ok(value.to_owned())
}
fn machine_name(value: String) -> Result<orchestrator_protocol::values::MessageText, Error> {
    if value.len() > 128 {
        return Err(Error::Invalid("machine_name_exceeds_128_bytes"));
    }
    value
        .try_into()
        .map_err(|_| Error::Invalid("invalid_machine_name"))
}
fn role_name(role: InstallationRole) -> &'static str {
    match role {
        InstallationRole::RouterAgent => "Router + agent",
        InstallationRole::Router => "Router only",
        InstallationRole::Agent => "Agent only",
    }
}

#[cfg(test)]
#[path = "../tests/internal/cli.rs"]
mod tests;
