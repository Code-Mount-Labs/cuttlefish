//! Process creation without parent-owned pipes or kill-on-drop semantics.

use std::{
    fmt::Debug,
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

use crate::model::LaunchSpec;

/// An owned child that can be reaped without blocking. Dropping it must not kill
/// the specialist: supervisor replacement is an expected operation.
pub trait ManagedProcess: Debug {
    /// OS process identity, when available, for matching a registered worker.
    fn pid(&self) -> Option<u32> {
        None
    }

    /// Check termination and reap the process if it exited.
    ///
    /// # Errors
    /// Reports an operating-system wait failure.
    fn try_wait(&mut self) -> io::Result<Option<bool>>;
}

impl ManagedProcess for Child {
    fn pid(&self) -> Option<u32> {
        Some(self.id())
    }
    fn try_wait(&mut self) -> io::Result<Option<bool>> {
        Child::try_wait(self).map(|status| status.map(|exit| exit.success()))
    }
}

/// Process launch boundary, allowing deterministic spawn/wait failure tests.
pub trait Launcher: Debug {
    /// The owned process handle.
    type Process: ManagedProcess;

    /// Launch exactly one worker from a durable launch permit.
    ///
    /// # Errors
    /// Reports invalid directories or OS spawn failures.
    fn spawn(&mut self, journal: &Path, spec: &LaunchSpec) -> io::Result<Self::Process>;
}

/// Launches the node binary's worker mode in its own process group.
#[derive(Debug)]
pub struct LocalLauncher {
    executable: PathBuf,
}

impl LocalLauncher {
    /// Select an explicit executable rather than searching an untrusted PATH.
    #[must_use]
    pub const fn new(executable: PathBuf) -> Self {
        Self { executable }
    }
}

impl Launcher for LocalLauncher {
    type Process = Child;

    fn spawn(&mut self, journal: &Path, spec: &LaunchSpec) -> io::Result<Child> {
        if spec.directory.canonicalize()? != spec.directory || !spec.directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace changed since registration",
            ));
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(journal.with_file_name(format!("{}.log", spec.command.target.session_id)))?;
        let mut command = Command::new(&self.executable);
        command
            .arg("worker")
            .arg("--journal")
            .arg(journal)
            .arg("--session")
            .arg(spec.command.target.session_id.to_string())
            .arg("--instance")
            .arg(&spec.instance_id)
            .current_dir(&spec.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        // Each platform separates the worker from the parent's console group.
        // No unsafe pre-exec hooks, inherited pipes, or kill-on-close job objects.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        command.spawn()
    }
}
