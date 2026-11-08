//! Durable self-update coordination. Downloads and restarts run in a detached
//! helper, never inside the HTTP request or the process being replaced.
mod catalog;
mod store;
mod worker;
use crate::Error;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

/// Persisted update lifecycle; interrupted active phases are resumable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// A durable request is awaiting the helper.
    Queued,
    /// Downloading and verifying the selected published release.
    Downloading,
    /// Activating the background service and checking its version.
    Restarting,
    /// The new service is responding with the expected version.
    Complete,
    /// Binaries are installed; this foreground process requires manual restart.
    RestartRequired,
    /// An explicit failure can be retried with the same job identity.
    Failed,
}
impl Phase {
    fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Downloading | Self::Restarting)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "Preparing update…",
            Self::Downloading => "Downloading and verifying…",
            Self::Restarting => "Restarting Cuttlefish…",
            Self::Complete => "Cuttlefish is up to date.",
            Self::RestartRequired => {
                "Update installed. Restart your foreground Cuttlefish process to finish."
            }
            Self::Failed => "Update needs attention. Run cuttlefish update again to retry.",
        }
    }
}
/// One durable update or restart request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Stable identity for recovery and UI tracking.
    pub id: Uuid,
    /// Exact published version, with a leading v.
    pub version: String,
    /// Restart-only requests do not install software.
    pub restart_only: bool,
    /// Current durable phase.
    pub phase: Phase,
    /// Redacted error, never credentials or raw subprocess output.
    pub error: Option<String>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    available: Option<String>,
    checked: bool,
    check_error: Option<String>,
    job: Option<Job>,
}

/// Safe dashboard view of update availability and progress.
#[derive(Debug, Serialize)]
pub struct Status {
    /// Version of the process responding to the request.
    pub current_version: String,
    /// Newest compatible published version found by the last explicit check.
    pub available_version: Option<String>,
    /// Whether a release check has completed.
    pub checked: bool,
    /// Failure of the last release check, if any.
    pub check_error: Option<String>,
    /// Whether the installation uses the managed release layout.
    pub supported: bool,
    /// Whether a native background service exists for this installation.
    pub automatic_restart: bool,
    /// Last durable operation.
    pub job: Option<Job>,
}
/// Update operation submitted by an authenticated local administrator.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Check the trusted release catalog without installing anything.
    Check,
    /// Install the exact version shown by a prior release check.
    Install {
        /// Selected release tag.
        version: String,
    },
    /// Retry an interrupted or failed operation with its original identity.
    Retry,
    /// Restart the configured service without changing application files.
    Restart,
}

/// Fallible descriptor operations stay separate from update orchestration.
#[derive(Debug)]
struct ProcessFiles {
    secure: fn(&std::fs::File) -> std::io::Result<()>,
    duplicate: fn(&std::fs::File) -> std::io::Result<std::fs::File>,
}
impl Default for ProcessFiles {
    fn default() -> Self {
        Self {
            #[cfg(unix)]
            secure: secure_log,
            #[cfg(not(unix))]
            secure: |_| Ok(()),
            duplicate: std::fs::File::try_clone,
        }
    }
}
#[cfg(unix)]
fn secure_log(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[derive(Debug)]
pub(crate) struct Manager {
    directory: PathBuf,
    executable: PathBuf,
    home: Option<PathBuf>,
    current: String,
    os: &'static str,
    process_files: ProcessFiles,
}
impl Manager {
    pub fn new(directory: &Path, executable: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            directory: directory.to_owned(),
            executable,
            home,
            os: std::env::consts::OS,
            process_files: ProcessFiles::default(),
            current: format!("v{}", env!("CARGO_PKG_VERSION")),
        }
    }
    fn store(&self) -> Result<store::Store, Error> {
        store::Store::open(&self.directory)
    }
    fn root(&self) -> Result<PathBuf, Error> {
        crate::installation::root(&self.executable)
    }
    fn service(&self) -> Result<Option<std::net::SocketAddr>, Error> {
        crate::service::address(&self.directory, self.home.as_deref(), self.os)
    }
    pub fn status(&self) -> Result<Status, Error> {
        let record = self.store()?.read()?;
        Ok(Status {
            current_version: self.current.clone(),
            available_version: record.available.filter(|tag| {
                catalog::version(tag)
                    .ok()
                    .zip(catalog::version(&self.current).ok())
                    .is_some_and(|(available, current)| available > current)
            }),
            checked: record.checked,
            check_error: record.check_error,
            supported: self.root().is_ok()
                && catalog::target(self.os, std::env::consts::ARCH).is_ok(),
            automatic_restart: self.service()?.is_some(),
            job: record.job,
        })
    }
    pub async fn act(&self, action: Action) -> Result<Status, Error> {
        match action {
            Action::Check => self.check(catalog::CATALOG).await?,
            other => self.enqueue(other)?,
        }
        self.resume()?;
        self.status()
    }
    async fn check(&self, url: &str) -> Result<(), Error> {
        self.root()?;
        let store = self.store()?;
        let Some(_lock) = store.lock()? else {
            return Ok(());
        };
        let mut record = store.read()?;
        if record.job.as_ref().is_some_and(|job| job.phase.active()) {
            return Ok(());
        }
        let result = async {
            let target = catalog::target(self.os, std::env::consts::ARCH)?;
            catalog::latest(catalog::fetch(url).await?, &self.current, target)
        }
        .await;
        match result {
            Ok(available) => {
                record.available = available;
                record.checked = true;
                record.check_error = None;
            }
            Err(error) => {
                record.check_error = Some(error.to_string());
            }
        }
        store.write(&record)
    }
    fn enqueue(&self, action: Action) -> Result<(), Error> {
        self.root()?;
        let store = self.store()?;
        let Some(_lock) = store.lock()? else {
            return Ok(());
        };
        let mut record = store.read()?;
        if record.job.as_ref().is_some_and(|job| job.phase.active()) {
            return Ok(());
        }
        match action {
            Action::Install { version } => {
                catalog::version(&version)?;
                if record.available.as_ref() != Some(&version) {
                    return Err(Error::Invalid("check_for_updates_before_installing"));
                }
                record.job = Some(Job {
                    id: Uuid::new_v4(),
                    version,
                    restart_only: false,
                    phase: Phase::Queued,
                    error: None,
                });
            }
            Action::Restart => {
                if self.service()?.is_none() {
                    return Err(Error::Invalid("background_service_required"));
                }
                record.job = Some(Job {
                    id: Uuid::new_v4(),
                    version: self.current.clone(),
                    restart_only: true,
                    phase: Phase::Queued,
                    error: None,
                });
            }
            Action::Retry => {
                let job = record
                    .job
                    .as_mut()
                    .ok_or(Error::Invalid("no_update_to_retry"))?;
                if !matches!(job.phase, Phase::Failed | Phase::RestartRequired) {
                    return Err(Error::Invalid("update_already_complete"));
                }
                job.phase = Phase::Queued;
                job.error = None;
            }
            Action::Check => return Err(Error::Invalid("invalid_update_operation")),
        }
        store.write(&record)
    }
    pub fn resume(&self) -> Result<(), Error> {
        let store = self.store()?;
        let Some(_lock) = store.lock()? else {
            return Ok(());
        };
        if !store.read()?.job.is_some_and(|job| job.phase.active()) {
            return Ok(());
        }
        let log = store.directory.join("helper.log");
        store::reject_link(&log)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)?;
        (self.process_files.secure)(&file)?;
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .arg("--state-dir")
            .arg(&self.directory)
            .arg("update-worker")
            .stdin(std::process::Stdio::null())
            .stdout((self.process_files.duplicate)(&file)?)
            .stderr(file);
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(())
    }
}

pub(crate) fn manager(directory: &Path) -> Result<Manager, Error> {
    discover(directory, std::env::current_exe)
}
fn discover(
    directory: &Path,
    executable: fn() -> std::io::Result<PathBuf>,
) -> Result<Manager, Error> {
    Ok(Manager::new(
        directory,
        executable()?,
        std::env::var_os("HOME").map(PathBuf::from),
    ))
}
pub(crate) async fn run_worker(manager: Manager) -> Result<(), Error> {
    worker::run(&manager, &worker::Native::default()).await
}
pub(crate) async fn run_cli(manager: Manager, check_only: bool) -> Result<(), Error> {
    cli(
        &manager,
        check_only,
        catalog::CATALOG,
        900,
        Duration::from_secs(1),
    )
    .await
}
trait Control {
    fn check<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>>;
    fn status(&self) -> Result<Status, Error>;
    fn enqueue(&self, action: Action) -> Result<(), Error>;
    fn resume(&self) -> Result<(), Error>;
}
impl Control for Manager {
    fn check<'a>(
        &'a self,
        url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + 'a>> {
        Box::pin(self.check(url))
    }
    fn status(&self) -> Result<Status, Error> {
        self.status()
    }
    fn enqueue(&self, action: Action) -> Result<(), Error> {
        self.enqueue(action)
    }
    fn resume(&self) -> Result<(), Error> {
        self.resume()
    }
}
async fn cli(
    manager: &dyn Control,
    check_only: bool,
    url: &str,
    attempts: usize,
    delay: Duration,
) -> Result<(), Error> {
    manager.check(url).await?;
    let status = manager.status()?;
    if let Some(error) = status.check_error {
        println!("{error}");
        return Err(Error::Dependency("update_check_failed"));
    }
    if check_only {
        println!("Installed: {}", status.current_version);
        println!(
            "Available: {}",
            status.available_version.as_deref().unwrap_or("up to date")
        );
        return Ok(());
    }
    if status
        .job
        .as_ref()
        .is_some_and(|job| matches!(job.phase, Phase::Failed | Phase::RestartRequired))
    {
        manager.enqueue(Action::Retry)?;
    } else if let Some(version) = status.available_version {
        manager.enqueue(Action::Install { version })?;
    } else if !status.job.is_some_and(|job| job.phase.active()) {
        println!("Cuttlefish is up to date.");
        return Ok(());
    }
    manager.resume()?;
    let mut previous = None;
    for _ in 0..attempts {
        if let Some(job) = manager.status()?.job {
            if previous != Some(job.phase) {
                println!("{}", job.phase.label());
                previous = Some(job.phase);
            }
            if !job.phase.active() {
                if job.phase == Phase::Failed {
                    return Err(Error::Dependency("update_failed_check_dashboard"));
                }
                return Ok(());
            }
        }
        tokio::time::sleep(delay).await;
    }
    Err(Error::Dependency("update_continues_check_dashboard"))
}

#[cfg(all(test, unix))]
#[path = "../../tests/internal/updates.rs"]
mod tests;
