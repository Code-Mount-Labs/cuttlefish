//! Group-serialized assignments, capacity reservations and durable delivery.
use crate::{
    Error,
    store::{Store, authenticate},
};
use orchestrator_protocol::{
    agent::{AgentAction, AgentCommand, AgentEventKind, SessionTarget},
    capabilities::Capability,
    ids::{GroupId, NodeId, ThreadId, WorkspaceId},
    routing::{
        Activity, Binding, Candidate, DeliveryState, DispatchBatch, DispatchPoll, DispatchView,
        RouteAction, RouteRequest, RoutingContext,
    },
    sync::{CommandOutcome, NodeToken, SyncEvent},
    values::{Epoch, ProtocolVersion},
};
use sqlx::{Postgres, Transaction, types::Json};
use uuid::Uuid;

type Tx<'a> = Transaction<'a, Postgres>;
type SavedBinding = (Json<Binding>, i64, bool);
const VIEW: &str = "SELECT jsonb_build_object('request',request,'command',command,'state',state,'outcome',outcome) FROM dispatches WHERE group_id=$1 AND command_id=$2";

#[derive(Debug, Clone)]
pub(crate) enum Input {
    Submit(RouteRequest),
    Failover(ThreadId),
}
#[derive(Debug)]
pub(crate) struct Plan {
    pub input: Input,
    pub context: RoutingContext,
    pub affinity: Option<NodeId>,
    assignment: Assignment,
    revision: Option<i64>,
}
#[derive(Debug)]
enum Assignment {
    Start {
        target: SessionTarget,
        workspace_id: WorkspaceId,
        capabilities: std::collections::BTreeSet<Capability>,
    },
    Resume(Binding),
    Replace(Binding),
}
#[derive(Debug)]
pub(crate) enum Prepared {
    Ready(Box<DispatchView>),
    Choose(Box<Plan>),
}

impl Store {
    pub(crate) async fn prepare(&self, token: &NodeToken, input: Input) -> Result<Prepared, Error> {
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        crate::collaboration::authorize_route(&mut tx, group, node, &input).await?;
        prepare_locked(&mut tx, group, input).await
    }

    pub(crate) async fn routing_transaction(
        &self,
        token: &NodeToken,
    ) -> Result<(Tx<'_>, Uuid, Uuid), Error> {
        let mut tx = self.pool.begin().await?;
        let (group, node) = authenticate(&mut tx, token).await?;
        sqlx::query("SELECT group_id FROM groups WHERE group_id=$1 FOR UPDATE")
            .bind(group)
            .fetch_one(&mut *tx)
            .await?;
        Ok((tx, group, node))
    }

    pub(crate) async fn commit_route(
        &self,
        token: &NodeToken,
        plan: Plan,
        node: NodeId,
    ) -> Result<DispatchView, Error> {
        let (mut tx, group, actor) = self.routing_transaction(token).await?;
        crate::collaboration::authorize_route(&mut tx, group, actor, &plan.input).await?;
        let fresh = match prepare_locked(&mut tx, group, plan.input.clone()).await? {
            Prepared::Ready(view) => return Ok(*view),
            Prepared::Choose(fresh) => fresh,
        };
        if fresh.revision != plan.revision {
            return Err(Error::RoutingConflict(
                "routing context changed; retry the same request",
            ));
        }
        if fresh.affinity.map_or_else(
            || !fresh.context.candidates.iter().any(|c| c.node_id == node),
            |owner| owner != node,
        ) {
            return Err(Error::NoCapacity);
        }
        let binding = make_binding(&fresh, node)?;
        let revision = fresh
            .revision
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::RoutingConflict("binding revision exhausted"))?;
        let request = &fresh.context.request;
        let command = AgentCommand {
            version: ProtocolVersion::V1,
            command_id: request.command_id,
            group_id: GroupId::try_from(group).map_err(|_| Error::Unauthorized)?,
            node_id: node,
            target: binding.target,
            action: match &request.action {
                RouteAction::Start { message, .. } | RouteAction::Restart { message, .. } => {
                    AgentAction::Start {
                        workspace_id: binding.workspace_id,
                        message: message.clone(),
                    }
                }
                RouteAction::Send { message } => AgentAction::Send {
                    message: message.clone(),
                },
                RouteAction::Cancel {} => AgentAction::Cancel,
            },
        };
        sqlx::query("INSERT INTO thread_bindings(group_id,thread_id,task_id,binding,revision) VALUES ($1,$2,$3,$4,$5) ON CONFLICT(group_id,thread_id) DO UPDATE SET binding=EXCLUDED.binding,revision=EXCLUDED.revision,stopped=FALSE")
            .bind(group).bind(Uuid::from(request.thread_id)).bind(Uuid::from(binding.target.task_id)).bind(Json(&binding)).bind(revision).execute(&mut *tx).await?;
        if matches!(fresh.input, Input::Failover(_)) {
            sqlx::query(
                "UPDATE dispatches SET command=$1,node_id=$2 WHERE group_id=$3 AND command_id=$4",
            )
            .bind(Json(&command))
            .bind(Uuid::from(node))
            .bind(group)
            .bind(Uuid::from(request.command_id))
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("INSERT INTO dispatches(group_id,thread_id,command_id,node_id,request,command,state) VALUES ($1,$2,$3,$4,$5,$6,'queued')").bind(group).bind(Uuid::from(request.thread_id)).bind(Uuid::from(request.command_id)).bind(Uuid::from(node)).bind(Json(request)).bind(Json(&command)).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(DispatchView {
            request: request.clone(),
            command,
            state: DeliveryState::Queued,
            outcome: None,
        })
    }

    /// Inspect the latest command for a conversation in the authenticated group.
    ///
    /// # Errors
    /// Reports unknown threads, authentication or storage failures.
    pub async fn thread(&self, token: &NodeToken, thread: ThreadId) -> Result<DispatchView, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        latest(&mut tx, group, thread).await
    }

    /// Atomically register execution readiness, acknowledge local acceptance,
    /// and claim a single command before exposing it to the network.
    ///
    /// # Errors
    /// Rejects oversized or inconsistent acknowledgments and storage failures.
    pub async fn poll_dispatch(
        &self,
        token: &NodeToken,
        poll: &DispatchPoll,
    ) -> Result<DispatchBatch, Error> {
        if poll.workspaces.len() > 1024
            || poll.accepted.len() > 32
            || poll
                .accepted
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != poll.accepted.len()
        {
            return Err(Error::RoutingConflict("invalid dispatch poll"));
        }
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        sqlx::query("UPDATE nodes SET dispatch_seen=clock_timestamp(),dispatch_active=$1,dispatch_workspaces=$2 WHERE node_id=$3").bind(i64::from(poll.active_sessions)).bind(Json(&poll.workspaces)).bind(node).execute(&mut *tx).await?;
        for id in &poll.accepted {
            let result=sqlx::query("UPDATE dispatches SET accepted=TRUE WHERE group_id=$1 AND node_id=$2 AND command_id=$3 AND state IN ('claimed','completed')").bind(group).bind(node).bind(Uuid::from(*id)).execute(&mut *tx).await?;
            if result.rows_affected() != 1 {
                return Err(Error::RoutingConflict(
                    "acceptance does not match a delivered command",
                ));
            }
        }
        let command:Option<Json<AgentCommand>>=sqlx::query_scalar("UPDATE dispatches SET state='claimed' WHERE sequence=(SELECT d.sequence FROM dispatches d JOIN nodes n ON n.node_id=d.node_id JOIN thread_bindings t ON t.group_id=d.group_id AND t.thread_id=d.thread_id WHERE d.group_id=$1 AND d.node_id=$2 AND (d.state='queued' OR d.state='claimed' AND NOT d.accepted) AND (d.state='claimed' OR d.command#>>'{action,type}'<>'start' OR (n.dispatch_active < (n.profile->>'capacity')::bigint AND n.dispatch_workspaces @> jsonb_build_array(d.command#>>'{action,workspace_id}') AND (n.profile->'capabilities') @> (t.binding->'capabilities'))) ORDER BY d.sequence LIMIT 1) RETURNING command")
            .bind(group).bind(node).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(DispatchBatch {
            acknowledged: poll.accepted.clone(),
            command: command.map(|c| c.0),
        })
    }
}

async fn latest(tx: &mut Tx<'_>, group: Uuid, thread: ThreadId) -> Result<DispatchView, Error> {
    let Json(view)=sqlx::query_scalar("SELECT jsonb_build_object('request',request,'command',command,'state',state,'outcome',outcome) FROM dispatches WHERE group_id=$1 AND thread_id=$2 ORDER BY sequence DESC LIMIT 1")
        .bind(group).bind(Uuid::from(thread)).fetch_optional(&mut **tx).await?.ok_or(Error::RoutingConflict("thread does not exist"))?;
    Ok(view)
}

async fn prepare_locked(tx: &mut Tx<'_>, group: Uuid, input: Input) -> Result<Prepared, Error> {
    let failover = matches!(input, Input::Failover(_));
    let request = match &input {
        Input::Submit(request) => request.clone(),
        Input::Failover(thread) => latest(tx, group, *thread).await?.request,
    };
    let receipt: Option<Json<DispatchView>> = sqlx::query_scalar(VIEW)
        .bind(group)
        .bind(Uuid::from(request.command_id))
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(Json(view)) = &receipt {
        if view.request != request {
            return Err(Error::RoutingConflict(
                "command ID conflicts with its saved request",
            ));
        }
        if !failover {
            return Ok(Prepared::Ready(Box::new(view.clone())));
        }
        if view.state != DeliveryState::Queued
            || !matches!(view.command.action, AgentAction::Start { .. })
        {
            return Err(Error::Unresolved);
        }
        let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE node_id=$1 AND NOT revoked AND last_seen>clock_timestamp()-interval '30 seconds' AND dispatch_seen>clock_timestamp()-interval '30 seconds')").bind(Uuid::from(view.command.node_id)).fetch_one(&mut **tx).await?;
        if live {
            return Ok(Prepared::Ready(Box::new(view.clone())));
        }
    }
    let saved: Option<SavedBinding> = sqlx::query_as(
        "SELECT binding,revision,stopped FROM thread_bindings WHERE group_id=$1 AND thread_id=$2",
    )
    .bind(group)
    .bind(Uuid::from(request.thread_id))
    .fetch_optional(&mut **tx)
    .await?;
    let assignment = assignment(&input, &request.action, saved.as_ref())?;
    if !failover {
        validate_request(tx, group, &request).await?;
    }
    let revision = saved.as_ref().map(|(_, revision, _)| *revision);
    let affinity = match &assignment {
        Assignment::Resume(binding) => Some(binding.node_id),
        _ => None,
    };
    let mut context = RoutingContext {
        active_work: Vec::new(),
        request,
        candidates: Vec::new(),
        recent_activity: Vec::new(),
    };
    let (workspace, capabilities, exclude) = match &assignment {
        Assignment::Start {
            workspace_id,
            capabilities,
            ..
        } => (*workspace_id, capabilities, None),
        Assignment::Replace(binding) | Assignment::Resume(binding) => (
            binding.workspace_id,
            &binding.capabilities,
            if failover {
                Some(Uuid::from(binding.node_id))
            } else {
                None
            },
        ),
    };
    if affinity.is_none() {
        let candidates:Vec<Json<Candidate>>=sqlx::query_scalar("SELECT jsonb_build_object('node_id',n.node_id,'profile',n.profile,'available_slots',(n.profile->>'capacity')::bigint-n.dispatch_active-(SELECT count(*) FROM dispatches d WHERE d.node_id=n.node_id AND NOT d.accepted AND d.command#>>'{action,type}'='start' AND d.command_id<>$4)) FROM nodes n WHERE n.group_id=$1 AND NOT n.revoked AND n.last_seen>clock_timestamp()-interval '30 seconds' AND n.dispatch_seen>clock_timestamp()-interval '30 seconds' AND n.dispatch_workspaces @> $2 AND n.profile->'capabilities' @> $3 AND ($5::uuid IS NULL OR n.node_id<>$5) AND (n.profile->>'capacity')::bigint>n.dispatch_active+(SELECT count(*) FROM dispatches d WHERE d.node_id=n.node_id AND NOT d.accepted AND d.command#>>'{action,type}'='start' AND d.command_id<>$4) ORDER BY n.dispatch_active,n.node_id LIMIT 32")
            .bind(group).bind(Json(vec![workspace])).bind(Json(capabilities)).bind(Uuid::from(context.request.command_id)).bind(exclude).fetch_all(&mut **tx).await?;
        context.candidates = candidates.into_iter().map(|c| c.0).collect();
        if context.candidates.is_empty() {
            return Err(Error::NoCapacity);
        }
        let activity:Vec<Json<Activity>>=sqlx::query_scalar("SELECT jsonb_build_object('node_id',node_id,'summary',left(COALESCE(payload#>>'{outcome,event,kind,text}',payload#>>'{outcome,reason}','session cancelled'),1000)) FROM node_events WHERE group_id=$1 ORDER BY cursor DESC LIMIT 8").bind(group).fetch_all(&mut **tx).await?;
        context.recent_activity = activity.into_iter().map(|a| a.0).collect();
        let work:Vec<Json<orchestrator_protocol::routing::GoalWork>>=sqlx::query_scalar("SELECT jsonb_build_object('goal_id',g.goal_id,'task_id',t.task_id,'owner',g.owner_node,'executor',d.node_id,'session_id',d.command#>>'{target,session_id}','state',COALESCE(d.state,CASE WHEN (s->>'dispatching')::boolean THEN 'reserved' ELSE 'planned' END),'summary',left(COALESCE(s#>>'{progress,note}',s#>>'{spec,acceptance}'),1000)) FROM goals g JOIN goal_tasks t ON t.group_id=g.group_id AND t.goal_id=g.goal_id CROSS JOIN LATERAL jsonb_array_elements(g.snapshot->'tasks') s LEFT JOIN dispatches d ON d.group_id=t.group_id AND d.command_id=t.command_id WHERE g.group_id=$1 AND g.snapshot->>'status'<>'completed' AND s#>>'{spec,request,command_id}'=t.command_id::text ORDER BY g.goal_id,t.task_id LIMIT 32").bind(group).fetch_all(&mut **tx).await?;
        context.active_work = work.into_iter().map(|w| w.0).collect();
    }
    Ok(Prepared::Choose(Box::new(Plan {
        input,
        context,
        affinity,
        assignment,
        revision,
    })))
}

async fn validate_request(
    tx: &mut Tx<'_>,
    group: Uuid,
    request: &RouteRequest,
) -> Result<(), Error> {
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dispatches WHERE group_id=$1 AND thread_id=$2 AND state<>'completed')").bind(group).bind(Uuid::from(request.thread_id)).fetch_one(&mut **tx).await?;
    if pending {
        return Err(Error::RoutingConflict(
            "wait for the outstanding command's result",
        ));
    }
    let identity = match &request.action {
        RouteAction::Start {
            task_id,
            session_id,
            ..
        } => Some((Some(*task_id), *session_id)),
        RouteAction::Restart { session_id, .. } => Some((None, *session_id)),
        _ => None,
    };
    if let Some((task, session)) = identity {
        let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dispatches WHERE group_id=$1 AND (command#>>'{target,session_id}'=$2 OR ($3::text IS NOT NULL AND command#>>'{target,task_id}'=$3)))")
            .bind(group).bind(session.to_string()).bind(task.map(|t|t.to_string())).fetch_one(&mut **tx).await?;
        if used {
            return Err(Error::RoutingConflict(
                "task or session identity is already used",
            ));
        }
    }
    Ok(())
}

fn assignment(
    input: &Input,
    action: &RouteAction,
    saved: Option<&SavedBinding>,
) -> Result<Assignment, Error> {
    if matches!(input, Input::Failover(_)) {
        return saved
            .map(|(binding, _, _)| Assignment::Replace(binding.0.clone()))
            .ok_or(Error::Unresolved);
    }
    match (action, saved) {
        (
            RouteAction::Start {
                task_id,
                session_id,
                workspace_id,
                capabilities,
                ..
            },
            None,
        ) if capabilities.len() <= 64 => Ok(Assignment::Start {
            target: SessionTarget {
                task_id: *task_id,
                session_id: *session_id,
                execution_epoch: Epoch::INITIAL,
            },
            workspace_id: *workspace_id,
            capabilities: capabilities.clone(),
        }),
        (RouteAction::Restart { session_id, .. }, Some((binding, _, true)))
            if *session_id != binding.target.session_id =>
        {
            let mut binding = binding.0.clone();
            binding.target.session_id = *session_id;
            Ok(Assignment::Replace(binding))
        }
        (RouteAction::Send { .. } | RouteAction::Cancel {}, Some((binding, _, false))) => {
            Ok(Assignment::Resume(binding.0.clone()))
        }
        _ => Err(Error::RoutingConflict(
            "operation does not match the thread lifecycle",
        )),
    }
}

fn make_binding(plan: &Plan, node: NodeId) -> Result<Binding, Error> {
    match &plan.assignment {
        Assignment::Replace(binding) => {
            let mut binding = binding.clone();
            binding.node_id = node;
            binding.target.execution_epoch = binding
                .target
                .execution_epoch
                .next()
                .map_err(|_| Error::RoutingConflict("execution epoch exhausted"))?;
            Ok(binding)
        }
        Assignment::Start {
            target,
            workspace_id,
            capabilities,
            ..
        } => Ok(Binding {
            node_id: node,
            target: *target,
            workspace_id: *workspace_id,
            capabilities: capabilities.clone(),
        }),
        Assignment::Resume(binding) => Ok(binding.clone()),
    }
}

pub(crate) async fn record_outcome(
    tx: &mut Tx<'_>,
    group: Uuid,
    event: &SyncEvent,
) -> Result<(), Error> {
    let saved: Option<Json<DispatchView>> = sqlx::query_scalar(VIEW)
        .bind(group)
        .bind(Uuid::from(event.command.command_id))
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(Json(view)) = saved {
        if view.command != event.command
            || view.state == DeliveryState::Queued
            || view
                .outcome
                .as_ref()
                .is_some_and(|outcome| *outcome != event.outcome)
        {
            return Err(Error::RoutingConflict(
                "result does not match a claimed dispatch",
            ));
        }
        sqlx::query("UPDATE dispatches SET state='completed',outcome=$1 WHERE group_id=$2 AND command_id=$3").bind(Json(&event.outcome)).bind(group).bind(Uuid::from(event.command.command_id)).execute(&mut **tx).await?;
        if matches!(&event.outcome,CommandOutcome::Completed {event} if event.kind==AgentEventKind::Cancelled)
        {
            sqlx::query("UPDATE thread_bindings SET stopped=TRUE WHERE group_id=$1 AND thread_id=$2 AND binding->'target'=$3").bind(group).bind(Uuid::from(view.request.thread_id)).bind(Json(&event.command.target)).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/internal/dispatch.rs"]
mod tests;
