//! Pure collaboration rules. External approval and execution observations are inputs.
use orchestrator_protocol::{
    agent::{AgentAction, AgentEventKind},
    collaboration::{
        CapabilityRelease, Discussion, GoalAction, GoalCommand, GoalSpec, GoalStatus, GoalView,
        Learning, LearningDecision, LearningStatus, TaskSpec, TaskView,
    },
    ids::{NodeId, ReleaseId, TaskId},
    routing::{DispatchView, RouteAction},
    sync::CommandOutcome,
};
use std::collections::BTreeSet;
use thiserror::Error;

/// A rejected collaboration mutation leaves the previous state unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct Rejected(pub &'static str);

/// Create an empty durable goal owned by its authenticated creator.
#[must_use]
pub fn create(spec: GoalSpec, owner: NodeId) -> GoalView {
    GoalView {
        spec,
        owner,
        revision: 0,
        status: GoalStatus::Active,
        tasks: vec![],
        discussion: vec![],
        learning: vec![],
        result: None,
    }
}

/// Extract a planned start's task identity.
///
/// # Errors
/// Plans may contain only new execution requests.
pub fn task_id(task: &TaskSpec) -> Result<TaskId, Rejected> {
    match task.request.action {
        RouteAction::Start { task_id, .. } => Ok(task_id),
        _ => Err(Rejected("planned tasks must start new executions")),
    }
}

/// Apply one operation atomically to a copy, checking authority and dependencies.
///
/// # Errors
/// Rejects stale revisions, unauthorized actions, invalid plans and lifecycle conflicts.
pub fn apply(
    view: &GoalView,
    actor: NodeId,
    command: &GoalCommand,
    catalog: &[CapabilityRelease],
) -> Result<GoalView, Rejected> {
    if view.revision != command.expected_revision {
        return Err(Rejected("goal revision changed"));
    }
    if view.status == GoalStatus::Completed {
        return Err(Rejected("goal is completed"));
    }
    let mut next = view.clone();
    match &command.action {
        GoalAction::Progress { task_id, progress } => {
            let task = find_mut(&mut next, *task_id)?;
            if task
                .execution
                .as_ref()
                .is_none_or(|e| e.command.node_id != actor)
            {
                return Err(Rejected("only the assigned executor can report progress"));
            }
            if task.verification.is_some()
                || progress.total == 0
                || progress.completed > progress.total
                || task
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.total != progress.total || p.completed > progress.completed)
            {
                return Err(Rejected("invalid or regressing progress"));
            }
            task.progress = Some(progress.clone());
        }
        GoalAction::Discuss { recipient, text } => {
            if !participant(view, actor) || recipient.is_some_and(|node| !participant(view, node)) {
                return Err(Rejected("discussion requires goal participation"));
            }
            if next.discussion.len() >= 128 {
                return Err(Rejected("discussion limit reached"));
            }
            next.discussion.push(Discussion {
                author: actor,
                recipient: *recipient,
                text: text.clone(),
            });
        }
        GoalAction::Evaluate {
            release_id,
            passed,
            evidence,
        } => {
            let learning = learning_mut(&mut next, *release_id, actor)?;
            if learning.node_id != actor
                || !matches!(
                    learning.status,
                    LearningStatus::Approved {} | LearningStatus::Evaluated { .. }
                )
            {
                return Err(Rejected(
                    "evaluation requires an approved acquisition on this node",
                ));
            }
            learning.status = LearningStatus::Evaluated {
                passed: *passed,
                evidence: evidence.clone(),
            };
        }
        action => {
            if actor != view.owner {
                return Err(Rejected("only the goal owner can change the plan"));
            }
            apply_owner(&mut next, action, catalog)?;
        }
    }
    advance(&mut next)?;
    Ok(next)
}

fn apply_owner(
    view: &mut GoalView,
    action: &GoalAction,
    catalog: &[CapabilityRelease],
) -> Result<(), Rejected> {
    match action {
        GoalAction::Plan { tasks } => {
            if tasks.is_empty() || view.tasks.len() + tasks.len() > 64 {
                return Err(Rejected("plan must contain between 1 and 64 tasks"));
            }
            view.tasks
                .extend(tasks.iter().cloned().map(|spec| TaskView {
                    spec,
                    dispatching: false,
                    execution: None,
                    progress: None,
                    verification: None,
                }));
            validate_plan(view)?;
        }
        GoalAction::Delegate { task_id: id } => {
            if view.status != GoalStatus::Active {
                return Err(Rejected("goal is paused"));
            }
            let verified: BTreeSet<TaskId> = view
                .tasks
                .iter()
                .filter(|t| t.verification.is_some())
                .filter_map(|t| task_id(&t.spec).ok())
                .collect();
            let task = find_mut(view, *id)?;
            if task.dispatching || !task.spec.dependencies.is_subset(&verified) {
                return Err(Rejected(
                    "task already delegated or dependencies not verified",
                ));
            }
            task.dispatching = true;
        }
        GoalAction::Verify { task_id, evidence } => {
            let task = find_mut(view, *task_id)?;
            if !task.execution.as_ref().is_some_and(successful) || task.verification.is_some() {
                return Err(Rejected(
                    "verification requires a successful unverified result",
                ));
            }
            task.verification = Some(evidence.clone());
        }
        GoalAction::Pause {} => view.status = GoalStatus::Paused,
        GoalAction::Resume {} => view.status = GoalStatus::Active,
        GoalAction::Finish { result } => {
            if view.tasks.is_empty() || view.tasks.iter().any(|t| t.verification.is_none()) {
                return Err(Rejected("all goal tasks must be verified"));
            }
            view.status = GoalStatus::Completed;
            view.result = Some(result.clone());
        }
        GoalAction::Learn { .. } | GoalAction::Replicate { .. } => {
            apply_learning(view, action, catalog)?;
        }
        _ => return Err(Rejected("operation is not an owner operation")),
    }
    Ok(())
}

fn apply_learning(
    view: &mut GoalView,
    action: &GoalAction,
    catalog: &[CapabilityRelease],
) -> Result<(), Rejected> {
    match action {
        GoalAction::Learn {
            task_id: id,
            node_id,
            release_id,
            rationale,
        } => {
            let task = find(view, *id)?;
            if task.execution.is_some()
                || view.learning.len() >= 32
                || view
                    .learning
                    .iter()
                    .any(|l| l.release.release_id == *release_id && l.node_id == *node_id)
            {
                return Err(Rejected(
                    "acquisition already exists or task has an execution",
                ));
            }
            let release = catalog
                .iter()
                .find(|r| r.release_id == *release_id)
                .ok_or(Rejected("release is not in the organization catalog"))?;
            if !matches!(&task.spec.request.action,RouteAction::Start {capabilities,..} if capabilities.contains(&release.capability))
            {
                return Err(Rejected("release does not supply a required capability"));
            }
            view.learning.push(Learning {
                task_id: *id,
                node_id: *node_id,
                release: release.clone(),
                rationale: rationale.clone(),
                status: LearningStatus::Proposed {},
            });
        }
        GoalAction::Replicate {
            release_id,
            nodes,
            rationale,
        } => {
            let learned = view
                .learning
                .iter()
                .find(|l| {
                    l.release.release_id == *release_id
                        && matches!(l.status, LearningStatus::Active {})
                })
                .ok_or(Rejected("replication requires an active learned release"))?
                .clone();
            if nodes.is_empty()
                || view.learning.len() + nodes.len() > 32
                || nodes.iter().any(|node| {
                    view.learning
                        .iter()
                        .any(|l| l.node_id == *node && l.release.release_id == *release_id)
                })
            {
                return Err(Rejected("invalid replication targets"));
            }
            for node in nodes {
                view.learning.push(Learning {
                    node_id: *node,
                    rationale: rationale.clone(),
                    status: LearningStatus::Proposed {},
                    ..learned.clone()
                });
            }
        }
        _ => return Err(Rejected("not a learning operation")),
    }
    Ok(())
}

/// Validate unique execution identities and an acyclic dependency graph.
///
/// # Errors
/// Rejects duplicate identities, unknown dependencies, cycles and non-start requests.
pub fn validate_plan(view: &GoalView) -> Result<(), Rejected> {
    let mut graph = Vec::new();
    let (mut ids, mut threads, mut commands, mut sessions) = (
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    for task in &view.tasks {
        let RouteAction::Start {
            task_id,
            session_id,
            capabilities,
            ..
        } = &task.spec.request.action
        else {
            return Err(Rejected("planned tasks must start new executions"));
        };
        if capabilities.len() > 64
            || !ids.insert(*task_id)
            || !threads.insert(task.spec.request.thread_id)
            || !commands.insert(task.spec.request.command_id)
            || !sessions.insert(*session_id)
            || task.spec.request.thread_id == view.spec.thread_id
        {
            return Err(Rejected(
                "plan contains duplicated or invalid execution identities",
            ));
        }
        graph.push((*task_id, &task.spec.dependencies));
    }
    let mut done = BTreeSet::new();
    loop {
        let before = done.len();
        for (id, dependencies) in &graph {
            if dependencies.is_subset(&done) {
                done.insert(*id);
            }
        }
        if done.len() == view.tasks.len() {
            return Ok(());
        }
        if before == done.len() {
            return Err(Rejected(
                "plan has a dependency cycle or unknown dependency",
            ));
        }
    }
}

/// Merge an execution observation without granting verification or changing ownership.
///
/// # Errors
/// Rejects observations for a different task request or an undelegated task.
pub fn observe(view: &mut GoalView, execution: DispatchView) -> Result<(), Rejected> {
    let task = view
        .tasks
        .iter_mut()
        .find(|t| t.spec.request.command_id == execution.request.command_id)
        .ok_or(Rejected("execution is outside this goal"))?;
    if !task.dispatching
        || execution.request != task.spec.request
        || !matches!(execution.command.action, AgentAction::Start { .. })
    {
        return Err(Rejected("execution does not match the planned delegation"));
    }
    task.execution = Some(execution);
    Ok(())
}

/// Resolve an operator decision against its exact reviewed revision.
///
/// # Errors
/// Rejects stale approvals and activation without a passing evaluation.
pub fn decide(
    view: &GoalView,
    revision: u64,
    release: ReleaseId,
    node: NodeId,
    decision: &LearningDecision,
) -> Result<GoalView, Rejected> {
    if view.revision != revision {
        return Err(Rejected("goal revision changed"));
    }
    let mut next = view.clone();
    let learning = learning_mut(&mut next, release, node)?;
    learning.status = match (decision, &learning.status) {
        (LearningDecision::Approve {}, LearningStatus::Proposed {}) => LearningStatus::Approved {},
        (LearningDecision::Activate {}, LearningStatus::Evaluated { passed: true, .. }) => {
            LearningStatus::Active {}
        }
        (LearningDecision::Reject { reason }, _) => LearningStatus::Rejected {
            reason: reason.clone(),
        },
        _ => return Err(Rejected("invalid learning approval transition")),
    };
    advance(&mut next)?;
    Ok(next)
}

/// Is this node an owner, executor, or approved learning participant?
#[must_use]
pub fn participant(view: &GoalView, node: NodeId) -> bool {
    view.owner == node
        || view.tasks.iter().any(|t| {
            t.execution
                .as_ref()
                .is_some_and(|e| e.command.node_id == node)
        })
        || view.learning.iter().any(|l| l.node_id == node)
}
/// Find a planned task.
///
/// # Errors
/// Rejects unknown task identities.
pub fn find(view: &GoalView, id: TaskId) -> Result<&TaskView, Rejected> {
    view.tasks
        .iter()
        .find(|t| task_id(&t.spec) == Ok(id))
        .ok_or(Rejected("unknown goal task"))
}
fn find_mut(view: &mut GoalView, id: TaskId) -> Result<&mut TaskView, Rejected> {
    view.tasks
        .iter_mut()
        .find(|t| task_id(&t.spec) == Ok(id))
        .ok_or(Rejected("unknown goal task"))
}
fn learning_mut(
    view: &mut GoalView,
    id: ReleaseId,
    node: NodeId,
) -> Result<&mut Learning, Rejected> {
    view.learning
        .iter_mut()
        .find(|l| l.release.release_id == id && l.node_id == node)
        .ok_or(Rejected("unknown learning request for node"))
}
fn advance(view: &mut GoalView) -> Result<(), Rejected> {
    view.revision = view
        .revision
        .checked_add(1)
        .ok_or(Rejected("goal revision exhausted"))?;
    Ok(())
}
fn successful(view: &DispatchView) -> bool {
    matches!(&view.outcome,Some(CommandOutcome::Completed {event}) if matches!(event.kind,AgentEventKind::Replied {..}))
}

#[cfg(test)]
#[path = "../tests/internal/collaboration.rs"]
mod tests;
