//! Machine-local setup journal, independent of the execution database.
use crate::{
    Error,
    resources::{DatabaseBackend, DatabaseChoice, DatabaseSpec, ResourceStatus},
};
use orchestrator_protocol::installation::Registration;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

/// One local installation's configuration and resource reconciliation boundary.
#[derive(Debug, Clone)]
pub struct Machine {
    pub(crate) pool: SqlitePool,
    directory: PathBuf,
    pub(crate) changes: Arc<Mutex<()>>,
    pub(crate) password_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) files: crate::files::AtomicFiles,
    pub(crate) passwords: crate::passwords::Passwords,
    backend: Arc<dyn DatabaseBackend>,
    _ownership: Arc<Ownership>,
    pub(crate) execution: Arc<tokio::sync::OnceCell<orchestrator_coordinator::store::Store>>,
}
impl Machine {
    /// Open the setup journal even when no execution database exists.
    ///
    /// # Errors
    /// Reports filesystem, migration, or local database failures.
    pub async fn open(directory: &Path, backend: Arc<dyn DatabaseBackend>) -> Result<Self, Error> {
        Self::open_with(directory, backend, protect).await
    }
    async fn open_with(
        directory: &Path,
        backend: Arc<dyn DatabaseBackend>,
        protect: fn(&Path, bool) -> std::io::Result<()>,
    ) -> Result<Self, Error> {
        reject_symlink(directory)?;
        std::fs::create_dir_all(directory)?;
        protect(directory, true)?;
        crate::settings::initialize_prompts(directory)?;
        reject_symlink(&directory.join("setup.lock"))?;
        reject_symlink(&directory.join("setup.sqlite"))?;
        let ownership = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("setup.lock"))?;
        ownership.try_lock().map_err(std::io::Error::from)?;
        let path = directory.join("setup.sqlite");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        protect(&path, false)?;
        drop(file);
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Full)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("UPDATE resources SET state='provisioning' WHERE state='ready'")
            .execute(&pool)
            .await?;
        Ok(Self {
            pool,
            directory: directory.to_owned(),
            changes: Arc::new(Mutex::new(())),
            password_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            files: crate::files::AtomicFiles::default(),
            passwords: crate::passwords::Passwords::default(),
            backend,
            _ownership: Arc::new(Ownership(ownership)),
            execution: Arc::new(tokio::sync::OnceCell::new()),
        })
    }
    /// Persist an exact installation identity once, before registration or provisioning.
    ///
    /// # Errors
    /// Rejects attempts to overwrite an existing identity or unavailable storage.
    pub async fn initialize(
        &self,
        registration: &Registration,
        platform_origin: &str,
        public_origin: &str,
    ) -> Result<(), Error> {
        crate::settings::validate_origin(platform_origin)?;
        crate::settings::validate_origin(public_origin)?;
        let saved: Option<i64> = sqlx::query_scalar("INSERT INTO installation(singleton,registration,platform_origin,public_origin) VALUES(1,?1,?2,?3) ON CONFLICT(singleton) DO UPDATE SET singleton=installation.singleton WHERE installation.registration=excluded.registration AND installation.platform_origin=excluded.platform_origin AND installation.public_origin=excluded.public_origin RETURNING singleton")
            .bind(sqlx::types::Json(registration)).bind(platform_origin).bind(public_origin).fetch_optional(&self.pool).await?;
        if saved.is_none() {
            return Err(Error::Invalid(
                "installation identity is already configured",
            ));
        }
        Ok(())
    }
    /// Read safe status for the local dashboard; no resource URL or password is exposed.
    ///
    /// # Errors
    /// Reports local storage failure.
    pub async fn status(&self) -> Result<ResourceStatus, Error> {
        let (revision, state, spec, error_code): (
            i64,
            String,
            Option<sqlx::types::Json<DatabaseSpec>>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT revision,state,spec,error_code FROM resources WHERE singleton=1",
        )
        .fetch_one(&self.pool)
        .await?;
        let mode = spec.map(|spec| match spec.0 {
            DatabaseSpec::Existing { .. } => "existing".into(),
            DatabaseSpec::Managed { .. } => "managed".into(),
        });
        Ok(ResourceStatus {
            revision,
            state,
            mode,
            error_code,
        })
    }
    /// Configure the primary database, or leave setup pending. A running primary
    /// cannot be silently replaced; migration requires a separate explicit workflow.
    ///
    /// # Errors
    /// Rejects invalid input, attempts to replace a selected resource, and failures.
    pub async fn configure(&self, choice: DatabaseChoice) -> Result<ResourceStatus, Error> {
        let _change = self.changes.lock().await;
        match self.select_locked(choice).await? {
            Some(spec) => self.reconcile_saved(&spec).await,
            None => self.status().await,
        }
    }
    /// Durably select a resource for the service's reconciliation worker.
    /// The request returns before downloads and provisioning start.
    ///
    /// # Errors
    /// Rejects conflicting selections, invalid input, or journal failure.
    pub async fn select_database(&self, choice: DatabaseChoice) -> Result<ResourceStatus, Error> {
        let _change = self.changes.lock().await;
        self.select_locked(choice).await?;
        self.status().await
    }
    /// Queue a failed resource for retry without tying work to an HTTP connection.
    ///
    /// # Errors
    /// Rejects missing selection, a ready resource, or journal failure.
    pub async fn retry_database(&self) -> Result<ResourceStatus, Error> {
        let _change = self.changes.lock().await;
        let status = self.status().await?;
        if !matches!(status.state.as_str(), "failed" | "provisioning") {
            return Err(Error::Invalid("database_has_no_pending_recovery"));
        }
        sqlx::query("UPDATE resources SET state='provisioning',error_code=NULL WHERE singleton=1")
            .execute(&self.pool)
            .await?;
        self.status().await
    }
    async fn select_locked(&self, choice: DatabaseChoice) -> Result<Option<DatabaseSpec>, Error> {
        let status = self.status().await?;
        let fixing_external = status.state == "failed"
            && status.mode.as_deref() == Some("existing")
            && matches!(choice, DatabaseChoice::Existing { .. });
        if status.state != "unconfigured" && !fixing_external {
            return Err(Error::Invalid(
                "database already selected; retry the saved configuration",
            ));
        }
        let spec = match choice {
            DatabaseChoice::Later => return Ok(None),
            DatabaseChoice::Existing { url } => {
                validate_url(&url)?;
                DatabaseSpec::Existing { url }
            }
            DatabaseChoice::Managed { port } => {
                if port == 0 {
                    return Err(Error::Invalid("database port must be nonzero"));
                }
                let available = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
                    .map_err(|_| Error::Invalid("database_port_is_already_in_use"))?;
                drop(available);
                let id = Uuid::new_v4().simple().to_string();
                DatabaseSpec::Managed {
                    container: format!("cuttlefish-pg-{id}"),
                    volume: format!("cuttlefish-pg-{id}"),
                    port,
                    password: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
                    admin_password: format!(
                        "{}{}",
                        Uuid::new_v4().simple(),
                        Uuid::new_v4().simple()
                    ),
                }
            }
        };
        sqlx::query("UPDATE resources SET revision=revision+1,state='provisioning',spec=?1,error_code=NULL WHERE singleton=1")
            .bind(sqlx::types::Json(&spec)).execute(&self.pool).await?;
        Ok(Some(spec))
    }
    /// Resume an interrupted or failed setup using the original saved credentials
    /// and volume identity. Never manufactures a replacement resource on retry.
    ///
    /// # Errors
    /// Reports unconfigured state, backend failure or unavailable local storage.
    pub async fn recover(&self) -> Result<ResourceStatus, Error> {
        let _change = self.changes.lock().await;
        let saved: Option<sqlx::types::Json<DatabaseSpec>> =
            sqlx::query_scalar("SELECT spec FROM resources WHERE singleton=1")
                .fetch_one(&self.pool)
                .await?;
        let spec = saved.ok_or(Error::Invalid("choose a database first"))?.0;
        sqlx::query("UPDATE resources SET state='provisioning',error_code=NULL WHERE singleton=1")
            .execute(&self.pool)
            .await?;
        self.reconcile_saved(&spec).await
    }
    async fn reconcile_saved(&self, spec: &DatabaseSpec) -> Result<ResourceStatus, Error> {
        let result = self.backend.reconcile(spec).await;
        let (state, error) = match &result {
            Ok(()) => ("ready", None),
            Err(Error::Dependency(code) | Error::Invalid(code)) => ("failed", Some(*code)),
            Err(_) => ("failed", Some("database_setup_failed")),
        };
        sqlx::query("UPDATE resources SET state=?1,error_code=?2 WHERE singleton=1")
            .bind(state)
            .bind(error)
            .execute(&self.pool)
            .await?;
        result?;
        self.status().await
    }
    /// Location of local private state.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Close journal connections before process shutdown.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
fn validate_url(value: &str) -> Result<(), Error> {
    let url = reqwest::Url::parse(value).map_err(|_| Error::Invalid("invalid PostgreSQL URL"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || url.host_str().is_none()
        || url.path().len() < 2
        || url.fragment().is_some()
    {
        return Err(Error::Invalid(
            "provide a PostgreSQL URL including host and database",
        ));
    }
    Ok(())
}
fn protect(path: &Path, directory: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        // Windows inherits access control from the user-owned state directory.
        std::fs::metadata(path).map(|_| ())
    }
}

fn reject_symlink(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local state must not be a symbolic link",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct Ownership(std::fs::File);
impl Drop for Ownership {
    fn drop(&mut self) {
        // Explicit release also covers a concurrent spawn temporarily inheriting
        // the file description before exec closes its descriptor.
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
#[path = "../tests/internal/startup.rs"]
mod tests;
