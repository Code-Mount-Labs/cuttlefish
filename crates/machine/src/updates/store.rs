//! Private atomic update journal and OS-held locks survive supervisor restarts.
use super::{Job, Record};
use crate::{Error, files::AtomicFiles};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

/// Explicit unlock prevents a concurrently forked child retaining ownership
/// between fork and exec after the original guard has been dropped.
#[derive(Debug)]
pub(super) struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Debug)]
pub(super) struct Store {
    pub directory: PathBuf,
    pub(super) files: AtomicFiles,
    encode: fn(&Record) -> Result<Vec<u8>, serde_json::Error>,
}
impl Store {
    pub fn open(state: &Path) -> Result<Self, Error> {
        Self::open_with(state, {
            #[cfg(unix)]
            {
                secure_directory
            }
            #[cfg(not(unix))]
            {
                |_| Ok(())
            }
        })
    }
    fn open_with(state: &Path, secure: fn(&Path) -> std::io::Result<()>) -> Result<Self, Error> {
        if !state.join("setup.sqlite").is_file() {
            return Err(Error::Invalid("installation_not_initialized"));
        }
        let directory = state.join("updates");
        reject_link(&directory)?;
        std::fs::create_dir_all(&directory)?;
        secure(&directory)?;
        let files = AtomicFiles {
            persist: |file, path| file.persist(path).map(|_| ()).map_err(|error| error.error),
            ..AtomicFiles::default()
        };
        Ok(Self {
            directory,
            files,
            encode: serde_json::to_vec,
        })
    }
    pub fn lock(&self) -> Result<Option<Lock>, Error> {
        lock(&self.directory.join("worker.lock"))
    }
    pub fn read(&self) -> Result<Record, Error> {
        let path = self.directory.join("status.json");
        reject_link(&path)?;
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Record::default()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn write(&self, record: &Record) -> Result<(), Error> {
        let path = self.directory.join("status.json");
        reject_link(&path)?;
        self.files
            .write_new(&self.directory, &path, &(self.encode)(record)?)?;
        Ok(())
    }
    pub fn save_job(&self, job: Job) -> Result<(), Error> {
        let mut record = self.read()?;
        record.job = Some(job);
        self.write(&record)
    }
}
#[cfg(unix)]
fn secure_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}
pub(super) fn lock(path: &Path) -> Result<Option<Lock>, Error> {
    lock_with(path, File::try_lock)
}
fn lock_with(
    path: &Path,
    acquire: fn(&File) -> Result<(), std::fs::TryLockError>,
) -> Result<Option<Lock>, Error> {
    reject_link(path)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    match acquire(&file) {
        Ok(()) => Ok(Some(Lock(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}
pub(super) fn reject_link(path: &Path) -> Result<(), Error> {
    if path.is_symlink() {
        return Err(Error::Invalid("update_path_must_not_be_symlink"));
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "../../tests/internal/update_store.rs"]
mod tests;
