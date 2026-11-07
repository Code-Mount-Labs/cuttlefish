//! Reconcile a private, persistent `PostgreSQL` container. Ownership labels and
//! the complete saved configuration are checked before adopting any container.
use crate::{
    Error,
    resources::{DatabaseBackend, DatabaseSpec, ResourceFuture},
};
use serde_json::Value;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;

/// Reviewed multi-platform official image, pinned instead of following latest.
pub const POSTGRES_IMAGE: &str =
    "postgres:18@sha256:e3d24f6906d0760bf4da0dae872415391ededefad23fd471c47dc53b0ddf3c42";
const OWNER: &str = "io.cuttlefish.resource";

/// Child command result; output may contain credentials and is never Debug-logged.
pub struct CommandResult {
    /// Whether the command exited successfully.
    pub success: bool,
    /// Captured stdout, used only for bounded structured inspection.
    pub stdout: Vec<u8>,
}
impl std::fmt::Debug for CommandResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandResult")
            .field("success", &self.success)
            .finish_non_exhaustive()
    }
}
/// Future for an external dependency operation.
pub type CommandFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CommandResult, Error>> + Send + 'a>>;
/// Narrow process boundary; arguments are never passed through a shell.
pub trait DockerCommands: std::fmt::Debug + Send + Sync {
    /// Ensure the selected Docker installation is available before provisioning.
    fn prepare(&self) -> ResourceFuture<'_> {
        Box::pin(async move {
            let result = self
                .execute(strings(&["info", "--format", "{{.ServerVersion}}"]))
                .await?;
            if !result.success {
                return Err(Error::Dependency("docker_unavailable"));
            }
            Ok(())
        })
    }
    /// Execute Docker with an operation deadline.
    fn execute(&self, args: Vec<String>) -> CommandFuture<'_>;
}
/// Invoke the locally installed Docker CLI.
#[derive(Debug, Default)]
pub struct SystemDocker;
impl DockerCommands for SystemDocker {
    fn prepare(&self) -> ResourceFuture<'_> {
        Box::pin(
            async move { ensure_available(self, crate::dependencies::install_docker(false)).await },
        )
    }

    fn execute(&self, args: Vec<String>) -> CommandFuture<'_> {
        Box::pin(async move {
            let mut command = Command::new(docker_binary());
            command.args(args);
            execute_bounded(command, Duration::from_mins(3)).await
        })
    }
}
/// Primary database verification and migration boundary.
pub trait DatabaseProbe: std::fmt::Debug + Send + Sync {
    /// Verify that the selected database is writable and has the execution schema.
    fn verify<'a>(&'a self, spec: &'a DatabaseSpec) -> ResourceFuture<'a>;
}
/// Verify the actual user-owned database before activating routing.
#[derive(Debug, Default)]
pub struct PostgresProbe;
impl DatabaseProbe for PostgresProbe {
    fn verify<'a>(&'a self, spec: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async move {
            if let DatabaseSpec::Managed {
                port,
                password,
                admin_password,
                ..
            } = spec
            {
                prepare_application_role(*port, password, admin_password).await?;
            }
            let url = spec.connection_url();
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(3))
                .connect(&url)
                .await?;
            let readonly: bool = sqlx::query_scalar(
                "SELECT pg_is_in_recovery() OR current_setting('transaction_read_only')='on'",
            )
            .fetch_one(&pool)
            .await?;
            pool.close().await;
            if readonly {
                return Err(Error::Invalid("primary_database_is_read_only"));
            }
            let store = orchestrator_coordinator::store::Store::open(&url)
                .await
                .map_err(|_| Error::Dependency("execution_schema_unavailable"))?;
            store.close().await;
            Ok(())
        })
    }
}
/// Local provisioning implementation. No daemon port or database password is
/// exposed to the network; agents connect to their router instead.
#[derive(Debug)]
pub struct DockerBackend {
    directory: PathBuf,
    commands: Arc<dyn DockerCommands>,
    probe: Arc<dyn DatabaseProbe>,
    files: crate::files::AtomicFiles,
}
impl DockerBackend {
    /// Use real Docker and `PostgreSQL` verification.
    #[must_use]
    pub fn new(directory: &Path) -> Self {
        Self::with_dependencies(directory, Arc::new(SystemDocker), Arc::new(PostgresProbe))
    }
    /// Supply explicit process/database boundaries, including deterministic test doubles.
    #[must_use]
    pub fn with_dependencies(
        directory: &Path,
        commands: Arc<dyn DockerCommands>,
        probe: Arc<dyn DatabaseProbe>,
    ) -> Self {
        Self {
            directory: directory.to_owned(),
            commands,
            probe,
            files: crate::files::AtomicFiles::default(),
        }
    }
    async fn command(&self, args: Vec<String>) -> Result<Vec<u8>, Error> {
        let result = self.commands.execute(args).await?;
        if !result.success {
            return Err(Error::Dependency("docker_operation_failed"));
        }
        Ok(result.stdout)
    }
    async fn ensure_managed(
        &self,
        container: &str,
        volume: &str,
        port: u16,
        password: &str,
    ) -> Result<(), Error> {
        validate_identity(container, volume, port, password)?;
        self.commands.prepare().await?;
        let volume_inspect = strings(&["volume", "inspect", volume]);
        if !self.commands.execute(volume_inspect.clone()).await?.success {
            self.command(strings(&[
                "volume",
                "create",
                "--label",
                &format!("{OWNER}={container}"),
                volume,
            ]))
            .await?;
        }
        let volume_json: Value = serde_json::from_slice(&self.command(volume_inspect).await?)?;
        if volume_json[0]["Labels"][OWNER] != container {
            return Err(Error::Invalid("volume_not_owned_by_this_installation"));
        }
        let inspect = strings(&["container", "inspect", container]);
        if !self.commands.execute(inspect.clone()).await?.success {
            let env_file = self.environment_file(container, password)?;
            self.command(strings(&[
                "container",
                "create",
                "--name",
                container,
                "--label",
                &format!("{OWNER}={container}"),
                "--restart",
                "unless-stopped",
                "--publish",
                &format!("127.0.0.1:{port}:5432"),
                "--mount",
                &format!("type=volume,source={volume},target=/var/lib/postgresql"),
                "--env-file",
                &env_file,
                POSTGRES_IMAGE,
            ]))
            .await?;
        }
        let saved: Value = serde_json::from_slice(&self.command(inspect).await?)?;
        verify_container(&saved[0], container, volume, port, password)?;
        if saved[0]["State"]["Running"] != true {
            self.command(strings(&["container", "start", container]))
                .await?;
        }
        Ok(())
    }
    fn environment_file(&self, container: &str, password: &str) -> Result<String, Error> {
        let path = self.directory.join(format!("{container}.env"));
        let expected = format!(
            "POSTGRES_USER=postgres\nPOSTGRES_DB=cuttlefish\nPOSTGRES_PASSWORD={password}\n"
        );
        if path.exists() {
            if std::fs::read(&path)? != expected.as_bytes() {
                return Err(Error::Invalid("saved_database_environment_changed"));
            }
        } else {
            self.files
                .write_new(&self.directory, &path, expected.as_bytes())?;
        }
        path_string(&path)
    }
}
impl DatabaseBackend for DockerBackend {
    fn reconcile<'a>(&'a self, spec: &'a DatabaseSpec) -> ResourceFuture<'a> {
        Box::pin(async move {
            if let DatabaseSpec::Managed {
                container,
                volume,
                port,
                admin_password,
                ..
            } = spec
            {
                self.ensure_managed(container, volume, *port, admin_password)
                    .await?;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_mins(1);
            loop {
                match self.probe.verify(spec).await {
                    Ok(()) => return Ok(()),
                    Err(Error::Database(_))
                        if matches!(spec, DatabaseSpec::Managed { .. })
                            && tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        })
    }
}
fn validate_identity(
    container: &str,
    volume: &str,
    port: u16,
    password: &str,
) -> Result<(), Error> {
    let suffix = container
        .strip_prefix("cuttlefish-pg-")
        .ok_or(Error::Invalid("invalid_managed_resource_identity"))?;
    if suffix.len() != 32
        || !suffix.bytes().all(|b| b.is_ascii_hexdigit())
        || container != volume
        || port == 0
        || password.len() != 64
        || !password.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::Invalid("invalid_managed_resource_identity"));
    }
    Ok(())
}
fn verify_container(
    saved: &Value,
    container: &str,
    volume: &str,
    port: u16,
    password: &str,
) -> Result<(), Error> {
    let bindings = &saved["HostConfig"]["PortBindings"]["5432/tcp"];
    let mount_ok = saved["Mounts"].as_array().is_some_and(|mounts| {
        mounts.iter().any(|m| {
            m["Type"] == "volume"
                && m["Name"] == volume
                && m["Destination"] == "/var/lib/postgresql"
        })
    });
    let env = &saved["Config"]["Env"];
    let env_ok = env.as_array().is_some_and(|entries| {
        [
            "POSTGRES_USER=postgres".to_owned(),
            "POSTGRES_DB=cuttlefish".to_owned(),
            format!("POSTGRES_PASSWORD={password}"),
        ]
        .iter()
        .all(|expected| entries.iter().any(|v| v.as_str() == Some(expected)))
    });
    if saved["Config"]["Labels"][OWNER] != container
        || saved["Config"]["Image"] != POSTGRES_IMAGE
        || saved["HostConfig"]["RestartPolicy"]["Name"] != "unless-stopped"
        || bindings.as_array().is_none_or(|b| b.len() != 1)
        || bindings[0]["HostIp"] != "127.0.0.1"
        || bindings[0]["HostPort"]
            .as_str()
            .and_then(|s| s.parse::<u16>().ok())
            != Some(port)
        || !mount_ok
        || !env_ok
    {
        return Err(Error::Invalid("existing_container_configuration_mismatch"));
    }
    Ok(())
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

async fn prepare_application_role(
    port: u16,
    password: &str,
    admin_password: &str,
) -> Result<(), Error> {
    // Passwords in this path are generated hexadecimal values, never arbitrary SQL.
    if [password, admin_password]
        .iter()
        .any(|value| value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(Error::Invalid("invalid_managed_database_secret"));
    }
    let url = format!("postgresql://postgres:{admin_password}@127.0.0.1:{port}/cuttlefish");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&url)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(78912964)")
        .execute(&mut *tx)
        .await?;
    let existing:Option<(bool,bool,bool,bool)>=sqlx::query_as("SELECT rolsuper,rolcreatedb,rolcreaterole,rolreplication FROM pg_roles WHERE rolname='cuttlefish'").fetch_optional(&mut *tx).await?;
    match existing {
        None => {
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE ROLE cuttlefish LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION"))).execute(&mut *tx).await?;
            sqlx::query("ALTER DATABASE cuttlefish OWNER TO cuttlefish")
                .execute(&mut *tx)
                .await?;
            sqlx::query("REVOKE CONNECT ON DATABASE cuttlefish FROM PUBLIC")
                .execute(&mut *tx)
                .await?;
        }
        Some((false, false, false, false)) => {}
        Some(_) => {
            return Err(Error::Invalid(
                "application_database_role_has_unexpected_privileges",
            ));
        }
    }
    tx.commit().await?;
    pool.close().await;
    Ok(())
}

fn docker_binary() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        let path =
            std::path::PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker");
        if path.is_file() {
            return path;
        }
    }
    #[cfg(windows)]
    {
        for (base, suffix) in [
            (
                "LOCALAPPDATA",
                "Programs/DockerDesktop/resources/bin/docker.exe",
            ),
            ("ProgramFiles", "Docker/Docker/resources/bin/docker.exe"),
        ] {
            if let Some(root) = std::env::var_os(base) {
                let path = std::path::PathBuf::from(root).join(suffix);
                if path.is_file() {
                    return path;
                }
            }
        }
    }
    std::path::PathBuf::from("docker")
}

async fn ensure_available(
    commands: &dyn DockerCommands,
    installation: impl Future<Output = Result<(), Error>>,
) -> Result<(), Error> {
    if commands
        .execute(strings(&["info", "--format", "{{.ServerVersion}}"]))
        .await
        .is_ok_and(|result| result.success)
    {
        return Ok(());
    }
    installation.await?;
    if !commands
        .execute(strings(&["info", "--format", "{{.ServerVersion}}"]))
        .await?
        .success
    {
        return Err(Error::Dependency(
            "docker_setup_requires_administrator_or_desktop_setup",
        ));
    }
    Ok(())
}

async fn execute_bounded(mut command: Command, deadline: Duration) -> Result<CommandResult, Error> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| Error::Dependency("docker_not_installed_or_not_accessible"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(Error::Dependency("docker_output_unavailable"))?;
    let operation = async {
        let mut bytes = Vec::new();
        stdout.take(1_048_577).read_to_end(&mut bytes).await?;
        if bytes.len() > 1_048_576 {
            return Err(Error::Dependency("docker_response_too_large"));
        }
        let status = child.wait().await?;
        Ok(CommandResult {
            success: status.success(),
            stdout: bytes,
        })
    };
    tokio::time::timeout(deadline, operation)
        .await
        .map_err(|_| Error::Dependency("docker_operation_timed_out"))?
}

#[cfg(test)]
#[path = "../tests/internal/docker.rs"]
mod tests;

fn path_string(path: &Path) -> Result<String, Error> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(Error::Invalid("installation_path_is_not_utf8"))
}
