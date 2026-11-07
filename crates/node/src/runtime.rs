//! Supervisor ownership, durable acceptance, and process reconciliation.

use std::{collections::BTreeMap, fs::File, io, path::Path};

use orchestrator_protocol::{agent::AgentCommand, ids::SessionId};
use uuid::Uuid;

use crate::{
    NodeError,
    journal::Journal,
    model::{CommandRecord, NodeIdentity},
    process::{Launcher, ManagedProcess},
};

/// Active supervisor for one node. The OS lock is released even on abrupt death;
/// specialists have independent handles and journal connections.
#[derive(Debug)]
pub struct NodeRuntime<L: Launcher> {
    journal: Journal,
    launcher: L,
    children: BTreeMap<SessionId, (String, L::Process)>,
    _owner_lock: File,
}

impl<L: Launcher> NodeRuntime<L> {
    /// Open the single supervisor for a node-local state directory.
    ///
    /// # Errors
    /// Reports directory, lock, migration, and identity errors.
    pub async fn open(
        directory: &Path,
        identity: NodeIdentity,
        launcher: L,
    ) -> Result<Self, NodeError> {
        let directory =
            std::fs::create_dir_all(directory).and_then(|()| directory.canonicalize())?;
        let owner_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("supervisor.lock"))?;
        owner_lock.try_lock().map_err(io::Error::from)?;
        let journal = Journal::open(&directory.join("node.sqlite"), identity).await?;
        Ok(Self {
            journal,
            launcher,
            children: BTreeMap::new(),
            _owner_lock: owner_lock,
        })
    }

    /// Access local records and workspace configuration.
    #[must_use]
    pub const fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Persist before spawning; existing receipts never authorize another spawn.
    ///
    /// # Errors
    /// Reports admission and launch failures. Even on error the caller must query
    /// the original command ID, not invent a new ID and replay unknown work.
    pub async fn submit(&mut self, command: &AgentCommand) -> Result<CommandRecord, NodeError> {
        let accepted = self.journal.accept(command, Uuid::new_v4()).await?;
        if let Some(spec) = accepted.launch {
            match self.launcher.spawn(self.journal.path(), &spec) {
                Ok(child) => {
                    self.children
                        .insert(spec.command.target.session_id, (spec.instance_id, child));
                }
                Err(error) => {
                    self.journal
                        .interrupted(
                            spec.command.target.session_id,
                            &spec.instance_id,
                            &format!("spawn failed: {error}"),
                        )
                        .await?;
                    return Err(error.into());
                }
            }
        }
        Ok(accepted.record)
    }

    /// Reap owned children and expire missing heartbeats, without killing or
    /// replacing any process based on a saved PID.
    ///
    /// # Errors
    /// Reports OS wait or journal failures.
    pub async fn tick(&mut self) -> Result<(), NodeError> {
        let mut exited = Vec::new();
        for (id, (instance, child)) in &mut self.children {
            if let Some(success) = child.try_wait()? {
                exited.push((*id, instance.clone(), success));
            }
        }
        for (id, instance, success) in exited {
            self.journal
                .interrupted(id, &instance, &format!("worker exited (success={success})"))
                .await?;
            self.children.remove(&id);
        }
        self.journal.expire(30_000).await
    }
}
