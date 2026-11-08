//! Per-user background service integration. launchd owns the process, while the
//! existing state directory remains the sole owner of identity and credentials.
use crate::Error;
use clap::Subcommand;
use std::{path::Path, time::Duration};

/// Explicit lifecycle operations for the local macOS login service.
#[derive(Debug, Clone, Copy, Subcommand)]
pub enum ServiceAction {
    /// Install and start a login service for this existing installation.
    Install,
    /// Start an installed service and enable automatic login startup.
    Start,
    /// Stop the service and disable automatic startup until started again.
    Stop,
    /// Restart an installed service without changing its saved configuration.
    Restart,
    /// Show whether the installed service is loaded and responding.
    Status,
    /// Remove automatic startup without deleting application data.
    Uninstall,
}
impl ServiceAction {
    fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Status => "status",
            Self::Uninstall => "uninstall",
        }
    }
}

/// Manage one installation's macOS `LaunchAgent`. No elevated account is used.
///
/// # Errors
/// Reports unsupported platforms, invalid paths, or failed service operations.
pub async fn manage(
    action: ServiceAction,
    directory: &Path,
    executable: &Path,
    listen: std::net::SocketAddr,
    home: Option<&std::ffi::OsStr>,
) -> Result<(), Error> {
    apply(
        action,
        directory,
        &crate::installation::stable(executable),
        listen,
        home,
        std::env::consts::OS,
        include_str!("../../../install/service-macos.sh"),
    )
    .await
}

pub(crate) async fn apply(
    action: ServiceAction,
    directory: &Path,
    executable: &Path,
    listen: std::net::SocketAddr,
    home: Option<&std::ffi::OsStr>,
    os: &str,
    script: &str,
) -> Result<(), Error> {
    if os != "macos" {
        return Err(Error::Invalid("background_service_requires_macos"));
    }
    if listen.port() == 0 {
        return Err(Error::Invalid("background_service_requires_fixed_port"));
    }
    let home = home.ok_or(Error::Invalid("background_service_requires_home"))?;
    let directory = std::fs::canonicalize(directory)?;
    let executable = std::path::absolute(executable)?;
    std::fs::metadata(&executable)?;
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-s", "--", action.name()])
        .arg(directory)
        .arg(executable)
        .arg(listen.to_string())
        .arg(listen.ip().to_string())
        .arg(listen.port().to_string())
        .arg(home);
    crate::dependencies::run_installer(command, script, true, Duration::from_secs(90))
        .await
        .map_err(|_| Error::Dependency("background_service_failed_see_message_above"))
}

#[cfg(test)]
#[path = "../tests/internal/service.rs"]
mod tests;

/// Read the saved listener for this installation's macOS login service.
///
/// # Errors
/// Reports unreadable or malformed service definitions.
pub fn address(
    directory: &Path,
    home: Option<&Path>,
    os: &str,
) -> Result<Option<std::net::SocketAddr>, Error> {
    use sha2::{Digest, Sha256};
    if os != "macos" {
        return Ok(None);
    }
    let Some(home) = home else {
        return Ok(None);
    };
    let directory = std::fs::canonicalize(directory)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(directory.as_os_str().as_encoded_bytes())
    );
    let path = home
        .join("Library/LaunchAgents")
        .join(format!("com.cuttlefish.app.{}.plist", &digest[..32]));
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    text.lines()
        .find_map(|line| {
            line.strip_prefix("<!-- Cuttlefish listener: ")
                .and_then(|value| value.strip_suffix(" -->"))
        })
        .ok_or(Error::Invalid("invalid_service_listener"))?
        .parse()
        .map(Some)
        .map_err(|_| Error::Invalid("invalid_service_listener"))
}
