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
}
/// Run setup or an existing local installation.
///
/// # Errors
/// Reports invalid setup input, network, storage and listener errors.
pub async fn run(cli: Cli) -> Result<(), Error> {
    if matches!(cli.command, Mode::InstallDocker) {
        return crate::dependencies::install_docker(true).await;
    }
    let directory = match cli.state_dir {
        Some(path) => path,
        None => PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .ok_or(Error::Invalid("provide --state-dir"))?,
        )
        .join(".cuttlefish"),
    };
    let machine = Machine::open(&directory, Arc::new(DockerBackend::new(&directory))).await?;
    let listen = match cli.command {
        Mode::Setup { platform, listen } => {
            if matches!(
                machine.identity().await,
                Err(Error::Invalid("installation_not_initialized"))
            ) {
                setup(
                    &machine,
                    &platform,
                    listen,
                    &mut io::stdin().lock(),
                    &mut io::stdout(),
                )
                .await?;
            }
            listen
        }
        Mode::Serve { listen } => listen,
        Mode::InstallDocker => return Err(Error::Invalid("unexpected setup mode")),
    };
    let identity = machine.identity().await?;
    println!("Cuttlefish dashboard: {}", identity.public_origin);
    if !machine.initialized().await? {
        println!(
            "One-time local setup code: {}",
            machine.bootstrap_token()?.expose()
        );
    }
    let worker = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "orchestrator-node.exe"
    } else {
        "orchestrator-node"
    });
    crate::runtime::serve(machine, listen, worker).await
}
async fn setup<R: BufRead, W: Write>(
    machine: &Machine,
    platform: &str,
    listen: SocketAddr,
    input: &mut R,
    output: &mut W,
) -> Result<(), Error> {
    let platform = validate_origin(platform)?;
    let group: GroupId = ask(input, output, "Group ID", None)?
        .parse()
        .map_err(|_| Error::Invalid("invalid_group_id"))?;
    let invitation = NodeToken::try_from(ask(input, output, "One-time enrollment code", None)?)
        .map_err(|_| Error::Invalid("invalid_enrollment_code"))?;
    let topology = Client::new(&platform)?
        .preflight(group, &invitation)
        .await?;
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
    let name = ask(input, output, "Machine name", Some("My machine"))?;
    if name.len() > 128 {
        return Err(Error::Invalid("machine_name_exceeds_128_bytes"));
    }
    let default_origin = format!("http://127.0.0.1:{}", listen.port());
    let public_origin = validate_origin(&ask(
        input,
        output,
        "Dashboard/router address (HTTPS for remote access)",
        Some(&default_origin),
    )?)?;
    let registration = Registration {
        group_id: group,
        installation_id: InstallationId::try_from(Uuid::new_v4())
            .map_err(|_| Error::Invalid("installation_identity_unavailable"))?,
        role,
        name: name
            .try_into()
            .map_err(|_| Error::Invalid("invalid_machine_name"))?,
        router_url: role.has_router().then(|| public_origin.clone()),
        credential: crate::auth::random_token()
            .try_into()
            .map_err(|_| Error::Invalid("installation_credential_unavailable"))?,
        invitation,
    };
    machine
        .initialize(&registration, &platform, &public_origin)
        .await?;
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
fn ask<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
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
