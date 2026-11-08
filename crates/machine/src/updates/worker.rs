//! Restart-independent update execution; active steps are safe to repeat.
use super::{Job, Manager, Phase, catalog, store};
use crate::Error;
use std::{future::Future, path::Path, pin::Pin, time::Duration};
type Work<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;
pub(super) trait Operations: std::fmt::Debug {
    fn install<'a>(&'a self, root: &'a Path, version: &'a str) -> Work<'a>;
    fn restart<'a>(
        &'a self,
        manager: &'a Manager,
        address: std::net::SocketAddr,
        version: &'a str,
    ) -> Work<'a>;
}
type Restart = for<'a> fn(&'a Manager, std::net::SocketAddr) -> Work<'a>;
#[derive(Debug)]
pub(super) struct Native {
    script: &'static str,
    restart: Restart,
}
impl Default for Native {
    fn default() -> Self {
        Self {
            script: include_str!("../../../../install.sh"),
            restart: restart_service,
        }
    }
}
fn restart_service(manager: &Manager, address: std::net::SocketAddr) -> Work<'_> {
    Box::pin(async move {
        crate::service::manage(
            crate::service::ServiceAction::Restart,
            &manager.directory,
            &crate::installation::stable(&manager.executable),
            address,
            manager.home.as_ref().map(|home| home.as_os_str()),
        )
        .await
    })
}
impl Operations for Native {
    fn install<'a>(&'a self, root: &'a Path, version: &'a str) -> Work<'a> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new("sh");
            command
                .args(["-s", "--"])
                .env("CUTTLEFISH_INSTALL_DIR", root)
                .env("CUTTLEFISH_VERSION", version)
                .env("CUTTLEFISH_NO_SETUP", "1");
            crate::dependencies::run_installer(command, self.script, true, Duration::from_mins(10))
                .await
                .map_err(|_| Error::Dependency("update_download_or_verification_failed"))
        })
    }
    fn restart<'a>(
        &'a self,
        manager: &'a Manager,
        address: std::net::SocketAddr,
        version: &'a str,
    ) -> Work<'a> {
        Box::pin(async move {
            (self.restart)(manager, address).await?;
            verify(&format!("http://{address}/api/local/status"), version).await
        })
    }
}
async fn verify(url: &str, version: &str) -> Result<(), Error> {
    verify_with(url, version, reqwest::Client::builder()).await
}
async fn verify_with(
    url: &str,
    version: &str,
    builder: reqwest::ClientBuilder,
) -> Result<(), Error> {
    let client = builder
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| Error::Dependency("updated_service_unavailable"))?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| Error::Dependency("updated_service_unavailable"))?;
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| Error::Dependency("updated_service_unavailable"))?;
    if value["version"].as_str() != Some(version.trim_start_matches('v')) {
        return Err(Error::Dependency("updated_service_version_mismatch"));
    }
    Ok(())
}

pub(super) async fn run(manager: &Manager, operations: &impl Operations) -> Result<(), Error> {
    let journal = manager.store()?;
    let mut lock = journal.lock()?;
    // The spawning parent briefly holds this lock while launching the helper.
    for _ in 0..20 {
        if lock.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        lock = journal.lock()?;
    }
    let Some(_lock) = lock else {
        return Ok(());
    };
    let Some(mut job) = journal.read()?.job.filter(|job| job.phase.active()) else {
        return Ok(());
    };
    let result = execute(manager, operations, &journal, &mut job).await;
    if let Err(error) = result {
        job.phase = Phase::Failed;
        job.error = Some(error.to_string());
        journal.save_job(job)?;
    }
    Ok(())
}
async fn execute(
    manager: &Manager,
    operations: &impl Operations,
    journal: &store::Store,
    job: &mut Job,
) -> Result<(), Error> {
    catalog::version(&job.version)?;
    let root = manager.root()?;
    let _lock = store::lock(&root.join("update.lock"))?
        .ok_or(Error::Invalid("another_installation_is_updating"))?;
    if !job.restart_only && matches!(job.phase, Phase::Queued | Phase::Downloading) {
        job.phase = Phase::Downloading;
        journal.save_job(job.clone())?;
        if std::fs::read_to_string(root.join("current/.release-version"))
            .ok()
            .as_deref()
            .map(str::trim)
            != Some(job.version.as_str())
        {
            operations.install(&root, &job.version).await?;
        }
    }
    if let Some(address) = manager.service()? {
        job.phase = Phase::Restarting;
        journal.save_job(job.clone())?;
        operations.restart(manager, address, &job.version).await?;
        job.phase = Phase::Complete;
    } else {
        job.phase = Phase::RestartRequired;
    }
    job.error = None;
    journal.save_job(job.clone())
}

#[cfg(all(test, unix))]
#[path = "../../tests/internal/update_worker.rs"]
mod tests;
