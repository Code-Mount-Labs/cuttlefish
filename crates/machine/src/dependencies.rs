//! Explicit dependency installation for the user-selected local database.
use crate::Error;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

/// Install or start Docker using the operating system's supported setup path.
/// Noninteractive dashboard calls never collect an OS administrator password.
///
/// # Errors
/// Reports missing administrator authorization, required OS setup, or installer failure.
pub async fn install_docker(interactive: bool) -> Result<(), Error> {
    #[cfg(unix)]
    let (mut command, script) = {
        let mut command = Command::new("sh");
        command.args([
            "-s",
            "--",
            if interactive {
                "--interactive"
            } else {
                "--non-interactive"
            },
        ]);
        (command, include_str!("../../../install/docker.sh"))
    };
    #[cfg(windows)]
    let (mut command, script) = {
        let _ = interactive;
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", "-"]);
        (command, include_str!("../../../install/docker.ps1"))
    };
    command.stdin(Stdio::piped()).kill_on_drop(true);
    if !interactive {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or(Error::Dependency("installer_input_unavailable"))?;
    input.write_all(script.as_bytes()).await?;
    drop(input);
    let status = tokio::time::timeout(Duration::from_mins(15), child.wait())
        .await
        .map_err(|_| Error::Dependency("docker_installation_timed_out"))??;
    if !status.success() {
        return Err(Error::Dependency(
            "docker_setup_requires_administrator_or_desktop_setup",
        ));
    }
    Ok(())
}
