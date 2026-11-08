//! Validated release layout. Only application files belong to this directory.
use crate::Error;
use std::path::{Path, PathBuf};

pub(crate) fn root(executable: &Path) -> Result<PathBuf, Error> {
    let executable = std::fs::canonicalize(executable)?;
    let root = executable
        .ancestors()
        .nth(3)
        .filter(|_| {
            executable
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name == "releases")
        })
        .ok_or(Error::Invalid("install_latest_bootstrap_to_enable_updates"))?;
    if std::fs::read(root.join(".cuttlefish-layout"))? != b"cuttlefish-release-layout-v1\n" {
        return Err(Error::Invalid("unrecognized_installation_layout"));
    }
    Ok(root.to_owned())
}
pub(crate) fn stable(executable: &Path) -> PathBuf {
    root(executable).map_or_else(
        |_| executable.to_owned(),
        |root| root.join("bin/cuttlefish"),
    )
}

#[cfg(test)]
#[path = "../tests/internal/installation.rs"]
mod tests;
