//! A specialist process owns its adapter for its entire lifetime, independent of
//! whichever supervisor currently accepts commands for the node.

use std::{convert::Infallible, time::Duration};

use orchestrator_agents::Agent;
use orchestrator_protocol::{
    agent::{AgentCommand, AgentEventKind},
    ids::SessionId,
};

use crate::{NodeError, journal::Journal, model::CommandOutcome};

/// Register and run one specialist. A process may claim its launch identity only
/// once, preventing accidental duplicate workers after a supervisor crash.
///
/// # Errors
/// Reports stale registration or storage failure. Adapter rejections are saved
/// as results; they are not silently retried.
pub async fn run<A: Agent>(
    journal: &Journal,
    session_id: SessionId,
    instance: &str,
    mut agent: A,
) -> Result<(), NodeError> {
    journal
        .register(session_id, instance, std::process::id())
        .await?;
    loop {
        let session = journal
            .session(session_id)
            .await?
            .ok_or(NodeError::Rejected("worker session disappeared"))?;
        if session.stop_requested {
            return journal.stopped(session_id, instance).await;
        }
        journal.heartbeat(session_id, instance).await?;
        if let Some(record) = journal.claim(session_id, instance).await? {
            let outcome = execute_command(journal, instance, &mut agent, &record.request).await?;
            journal
                .complete(record.request.command_id, instance, &outcome)
                .await?;
            if matches!(outcome, CommandOutcome::Completed { ref event } if event.kind == AgentEventKind::Cancelled)
            {
                return journal.stopped(session_id, instance).await;
            }
        } else {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

async fn execute_command<A: Agent>(
    journal: &Journal,
    instance: &str,
    agent: &mut A,
    command: &AgentCommand,
) -> Result<CommandOutcome, NodeError> {
    // Keep liveness independent of adapter latency. Cancel this heartbeat loop
    // before persisting completion, so it cannot race a confirmed session stop.
    let result = tokio::select! {
        result = agent.execute(command) => result,
        result = keep_alive(journal, command.target.session_id, instance) => return match result {
            Err(error) => Err(error),
            Ok(never) => match never {},
        },
    };
    finish_execution(
        journal,
        instance,
        command,
        result.map_err(|error| error.to_string()),
        agent.failure_is_uncertain(),
    )
    .await
}

async fn finish_execution(
    journal: &Journal,
    instance: &str,
    command: &AgentCommand,
    result: Result<orchestrator_protocol::agent::AgentEvent, String>,
    uncertain: bool,
) -> Result<CommandOutcome, NodeError> {
    match result {
        Ok(event) => Ok(CommandOutcome::Completed {
            event: Box::new(event),
        }),
        Err(error) => {
            if uncertain {
                journal
                    .interrupted(command.target.session_id, instance, &error)
                    .await?;
                return Err(NodeError::Rejected(
                    "provider execution uncertain; automatic replay refused",
                ));
            }
            Ok(CommandOutcome::Rejected { reason: error })
        }
    }
}

async fn keep_alive(
    journal: &Journal,
    session_id: SessionId,
    instance: &str,
) -> Result<Infallible, NodeError> {
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        journal.heartbeat(session_id, instance).await?;
    }
}

#[cfg(test)]
#[path = "../tests/internal/worker.rs"]
mod tests;
