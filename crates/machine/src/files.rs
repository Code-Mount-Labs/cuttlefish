//! Crash-safe creation of private files, shared by setup and database provisioning.
use std::{
    io::{self, Write},
    path::Path,
};
use tempfile::NamedTempFile;

/// Narrow filesystem operations allow deterministic disk-failure verification.
#[derive(Debug, Clone)]
pub(crate) struct AtomicFiles {
    pub create: fn(&Path) -> io::Result<NamedTempFile>,
    pub write: fn(&mut NamedTempFile, &[u8]) -> io::Result<()>,
    pub sync: fn(&NamedTempFile) -> io::Result<()>,
    pub persist: fn(NamedTempFile, &Path) -> io::Result<()>,
    pub sync_directory: fn(&Path) -> io::Result<()>,
}
impl Default for AtomicFiles {
    fn default() -> Self {
        Self {
            create: |directory| NamedTempFile::new_in(directory),
            write: |file, bytes| file.write_all(bytes),
            sync: |file| file.as_file().sync_all(),
            persist: |file, path| {
                file.persist_noclobber(path)
                    .map(|_| ())
                    .map_err(|error| error.error)
            },
            #[cfg(unix)]
            sync_directory: |path| std::fs::File::open(path).and_then(|file| file.sync_all()),
            #[cfg(not(unix))]
            sync_directory: |_| Ok(()),
        }
    }
}
impl AtomicFiles {
    pub fn write_new(&self, directory: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = (self.create)(directory)?;
        (self.write)(&mut file, bytes)?;
        (self.sync)(&file)?;
        (self.persist)(file, target)?;
        (self.sync_directory)(directory)
    }
}
#[cfg(test)]
#[path = "../tests/internal/files.rs"]
mod tests;
