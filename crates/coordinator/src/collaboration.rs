//! Durable collaboration and an organization catalog. No execution or installation.
use crate::{Error, dispatch::Input, store::Store};
use orchestrator_domain::collaboration::{self as policy, Rejected};
use orchestrator_protocol::{
    collaboration::{
        CapabilityRelease, GoalAction, GoalCommand, GoalSpec, GoalStatus, GoalView,
        LearningDecision,
    },
    ids::{GoalId, GroupId, NodeId, ReleaseId, TaskId},
    routing::{DispatchView, RouteAction, RouteRequest},
    sync::NodeToken,
};
use sqlx::{Postgres, Transaction, types::Json};
use uuid::Uuid;
type Tx<'a> = Transaction<'a, Postgres>;

impl Store {
    /// Read reviewed, active deployments for reuse across goals.
    ///
    /// # Errors
    /// Reports authentication, storage or decoding failure.
    pub async fn installations(
        &self,
        token: &NodeToken,
    ) -> Result<Vec<orchestrator_protocol::collaboration::InstalledCapability>, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        let rows:Vec<Json<orchestrator_protocol::collaboration::InstalledCapability>>=sqlx::query_scalar("SELECT jsonb_build_object('node_id',n.node_id,'release',r.manifest,'evidence',n.evidence) FROM node_capabilities n JOIN capability_releases r ON r.group_id=n.group_id AND r.release_id=n.release_id JOIN nodes p ON p.group_id=n.group_id AND p.node_id=n.node_id WHERE n.group_id=$1 AND n.active AND NOT p.revoked ORDER BY n.node_id,n.capability LIMIT 1000").bind(group).fetch_all(&mut *tx).await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }
    /// Create a goal idempotently in the authenticated group.
    ///
    /// # Errors
    /// Rejects identity reuse with different content and storage failures.
    pub async fn create_goal(&self, token: &NodeToken, spec: &GoalSpec) -> Result<GoalView, Error> {
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        let owner = NodeId::try_from(node).map_err(|_| Error::Unauthorized)?;
        let view = policy::create(spec.clone(), owner);
        let Json(saved):Json<GoalView>=sqlx::query_scalar("INSERT INTO goals(group_id,goal_id,thread_id,owner_node,snapshot) VALUES ($1,$2,$3,$4,$5) ON CONFLICT(group_id,goal_id) DO UPDATE SET snapshot=goals.snapshot RETURNING snapshot")
            .bind(group).bind(Uuid::from(spec.goal_id)).bind(Uuid::from(spec.thread_id)).bind(node).bind(Json(&view)).fetch_one(&mut *tx).await?;
        if saved.spec != *spec || saved.owner != owner {
            return Err(Rejected("goal identity conflict").into());
        }
        tx.commit().await?;
        Ok(saved)
    }
    /// Read a goal including its current specialist execution receipts.
    ///
    /// # Errors
    /// Rejects unknown goals, credentials and unavailable storage.
    pub async fn goal(&self, token: &NodeToken, id: GoalId) -> Result<GoalView, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        load(&mut tx, group, id).await
    }
    /// Read bounded goal snapshots for router context and restart discovery.
    ///
    /// # Errors
    /// Reports authentication, decoding or storage failure.
    pub async fn goals(&self, token: &NodeToken) -> Result<Vec<GoalView>, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT goal_id FROM goals WHERE group_id=$1 ORDER BY goal_id LIMIT 100",
        )
        .bind(group)
        .fetch_all(&mut *tx)
        .await?;
        let mut views = Vec::new();
        for id in ids {
            views.push(
                load(
                    &mut tx,
                    group,
                    id.try_into()
                        .map_err(|_| Rejected("invalid goal identity"))?,
                )
                .await?,
            );
        }
        Ok(views)
    }
    /// Apply one durable operation; exact retries return the original response.
    ///
    /// # Errors
    /// Rejects stale revisions, unauthorized mutations, identity conflicts and storage errors.
    pub async fn goal_command(
        &self,
        token: &NodeToken,
        id: GoalId,
        command: &GoalCommand,
    ) -> Result<GoalView, Error> {
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        let actor = node.to_string();
        let saved:Option<(Uuid,String,Json<GoalCommand>,Json<GoalView>)>=sqlx::query_as("SELECT goal_id,actor,request,response FROM goal_commands WHERE group_id=$1 AND command_id=$2").bind(group).bind(Uuid::from(command.command_id)).fetch_optional(&mut *tx).await?;
        if let Some((goal, author, request, response)) = saved {
            if goal != Uuid::from(id) || author != actor || request.0 != *command {
                return Err(Rejected("goal command identity conflict").into());
            }
            return Ok(response.0);
        }
        let view = load(&mut tx, group, id).await?;
        let catalog = catalog(&mut tx, group).await?;
        let next = policy::apply(
            &view,
            node.try_into().map_err(|_| Error::Unauthorized)?,
            command,
            &catalog,
        )?;
        validate_nodes(&mut tx, group, &command.action).await?;
        if let GoalAction::Plan { tasks } = &command.action {
            let linked=sqlx::query("INSERT INTO goal_tasks(group_id,goal_id,task_id,thread_id,command_id) SELECT $1,$2,(s#>>'{request,action,task_id}')::uuid,(s#>>'{request,thread_id}')::uuid,(s#>>'{request,command_id}')::uuid FROM jsonb_array_elements($3) s WHERE NOT EXISTS (SELECT 1 FROM dispatches d WHERE d.group_id=$1 AND (d.thread_id=(s#>>'{request,thread_id}')::uuid OR d.command_id=(s#>>'{request,command_id}')::uuid OR d.command#>>'{target,task_id}'=s#>>'{request,action,task_id}' OR d.command#>>'{target,session_id}'=s#>>'{request,action,session_id}'))").bind(group).bind(Uuid::from(id)).bind(Json(tasks)).execute(&mut *tx).await?;
            if linked.rows_affected() != tasks.len() as u64 {
                return Err(Rejected("plan reuses an existing execution identity").into());
            }
        }
        save(&mut tx, group, &next).await?;
        sqlx::query("INSERT INTO goal_commands(group_id,goal_id,command_id,actor,request,response) VALUES ($1,$2,$3,$4,$5,$6)").bind(group).bind(Uuid::from(id)).bind(Uuid::from(command.command_id)).bind(actor).bind(Json(command)).bind(Json(&next)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(next)
    }
    /// Resolve the immutable request for a previously reserved task.
    ///
    /// # Errors
    /// Rejects non-owners, unreserved work, paused goals and unknown tasks.
    pub async fn delegation(
        &self,
        token: &NodeToken,
        id: GoalId,
        task: TaskId,
    ) -> Result<RouteRequest, Error> {
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        let view = load(&mut tx, group, id).await?;
        if Uuid::from(view.owner) != node || view.status != GoalStatus::Active {
            return Err(Rejected("goal is not active or caller is not its owner").into());
        }
        let task = policy::find(&view, task)?;
        if !task.dispatching {
            return Err(Rejected("task has not been reserved for dispatch").into());
        }
        Ok(task.spec.request.clone())
    }
    /// Read the organization's immutable capability catalog.
    ///
    /// # Errors
    /// Reports authorization and storage failures.
    pub async fn catalog(&self, token: &NodeToken) -> Result<Vec<CapabilityRelease>, Error> {
        let (mut tx, group, _) = self.routing_transaction(token).await?;
        catalog(&mut tx, group).await
    }
    /// Operator-only catalog publication through database credentials, not a node API.
    ///
    /// # Errors
    /// Rejects changed releases, invalid predecessors and storage errors.
    pub async fn publish_release(
        &self,
        group: GroupId,
        release: &CapabilityRelease,
    ) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        lock_group(&mut tx, group).await?;
        let releases = catalog(&mut tx, group.into()).await?;
        if release.supersedes.is_some_and(|id| {
            !releases
                .iter()
                .any(|r| r.release_id == id && r.capability == release.capability)
        }) {
            return Err(Rejected("predecessor must exist and supply the same capability").into());
        }
        let Json(saved):Json<CapabilityRelease>=sqlx::query_scalar("INSERT INTO capability_releases(group_id,release_id,manifest) VALUES ($1,$2,$3) ON CONFLICT(group_id,release_id) DO UPDATE SET manifest=capability_releases.manifest RETURNING manifest").bind(Uuid::from(group)).bind(Uuid::from(release.release_id)).bind(Json(release)).fetch_one(&mut *tx).await?;
        if saved != *release {
            return Err(Rejected("catalog releases are immutable").into());
        }
        tx.commit().await?;
        Ok(())
    }
    /// Operator-only approval/revocation, scoped to an exact goal, node and release.
    ///
    /// # Errors
    /// Rejects stale reviews, invalid transitions and unavailable storage.
    pub async fn decide_learning(
        &self,
        group: GroupId,
        id: GoalId,
        revision: u64,
        node: NodeId,
        release: ReleaseId,
        decision: &LearningDecision,
    ) -> Result<GoalView, Error> {
        let mut tx = self.pool.begin().await?;
        lock_group(&mut tx, group).await?;
        let view = load(&mut tx, group.into(), id).await?;
        let next = policy::decide(&view, revision, release, node, decision)?;
        record_installation(&mut tx, group, node, release, decision, &view).await?;
        save(&mut tx, group.into(), &next).await?;
        sqlx::query("INSERT INTO goal_commands(group_id,goal_id,command_id,actor,request,response) VALUES ($1,$2,$3,'operator',$4,$5)").bind(Uuid::from(group)).bind(Uuid::from(id)).bind(Uuid::new_v4()).bind(Json(serde_json::json!({"node_id":node,"release_id":release,"revision":revision,"decision":decision}))).bind(Json(&next)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(next)
    }
}

async fn record_installation(
    tx: &mut Tx<'_>,
    group: GroupId,
    node: NodeId,
    release: ReleaseId,
    decision: &LearningDecision,
    view: &GoalView,
) -> Result<(), Error> {
    use orchestrator_protocol::collaboration::LearningStatus;
    match decision {
        LearningDecision::Activate {} => {
            // The domain transition already established the matching passing evaluation.
            for learning in &view.learning {
                if learning.node_id == node
                    && learning.release.release_id == release
                    && let LearningStatus::Evaluated { evidence, .. } = &learning.status
                {
                    sqlx::query("INSERT INTO node_capabilities(group_id,node_id,capability,release_id,evidence,active) VALUES($1,$2,$3,$4,$5,TRUE) ON CONFLICT(group_id,node_id,capability) DO UPDATE SET release_id=EXCLUDED.release_id,evidence=EXCLUDED.evidence,active=TRUE")
                            .bind(Uuid::from(group)).bind(Uuid::from(node)).bind(learning.release.capability.as_str()).bind(Uuid::from(release)).bind(Json(evidence)).execute(&mut **tx).await?;
                }
            }
        }
        LearningDecision::Reject { .. } => {
            sqlx::query("UPDATE node_capabilities SET active=FALSE WHERE group_id=$1 AND node_id=$2 AND release_id=$3").bind(Uuid::from(group)).bind(Uuid::from(node)).bind(Uuid::from(release)).execute(&mut **tx).await?;
        }
        LearningDecision::Approve {} => {}
    }
    Ok(())
}
async fn lock_group(tx: &mut Tx<'_>, group: GroupId) -> Result<(), Error> {
    sqlx::query("SELECT group_id FROM groups WHERE group_id=$1 FOR UPDATE")
        .bind(Uuid::from(group))
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}
async fn catalog(tx: &mut Tx<'_>, group: Uuid) -> Result<Vec<CapabilityRelease>, Error> {
    let rows: Vec<Json<CapabilityRelease>> = sqlx::query_scalar(
        "SELECT manifest FROM capability_releases WHERE group_id=$1 ORDER BY release_id LIMIT 1000",
    )
    .bind(group)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}
async fn load(tx: &mut Tx<'_>, group: Uuid, id: GoalId) -> Result<GoalView, Error> {
    let Json(mut view): Json<GoalView> =
        sqlx::query_scalar("SELECT snapshot FROM goals WHERE group_id=$1 AND goal_id=$2")
            .bind(group)
            .bind(Uuid::from(id))
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(Rejected("unknown goal"))?;
    let rows:Vec<Json<DispatchView>>=sqlx::query_scalar("SELECT jsonb_build_object('request',d.request,'command',d.command,'state',d.state,'outcome',d.outcome) FROM dispatches d JOIN goal_tasks t ON t.group_id=d.group_id AND t.command_id=d.command_id WHERE t.group_id=$1 AND t.goal_id=$2").bind(group).bind(Uuid::from(id)).fetch_all(&mut **tx).await?;
    for row in rows {
        policy::observe(&mut view, row.0)?;
    }
    Ok(view)
}
async fn save(tx: &mut Tx<'_>, group: Uuid, view: &GoalView) -> Result<(), Error> {
    sqlx::query("UPDATE goals SET snapshot=$1 WHERE group_id=$2 AND goal_id=$3")
        .bind(Json(view))
        .bind(group)
        .bind(Uuid::from(view.spec.goal_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
async fn validate_nodes(tx: &mut Tx<'_>, group: Uuid, action: &GoalAction) -> Result<(), Error> {
    let nodes: Vec<Uuid> = match action {
        GoalAction::Learn { node_id, .. } => vec![(*node_id).into()],
        GoalAction::Replicate { nodes, .. } => nodes.iter().map(|n| (*n).into()).collect(),
        _ => vec![],
    };
    let valid:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM unnest($2::uuid[]) id WHERE NOT EXISTS(SELECT 1 FROM nodes n WHERE n.group_id=$1 AND n.node_id=id AND NOT n.revoked))").bind(group).bind(nodes).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(Rejected("learning targets must be active members of this group").into());
    }
    Ok(())
}

pub(crate) async fn authorize_route(
    tx: &mut Tx<'_>,
    group: Uuid,
    node: Uuid,
    input: &Input,
) -> Result<(), Error> {
    let thread = match input {
        Input::Submit(request) => request.thread_id,
        Input::Failover(thread) => *thread,
    };
    let saved:Option<Json<GoalView>>=sqlx::query_scalar("SELECT g.snapshot FROM goals g JOIN goal_tasks t ON t.group_id=g.group_id AND t.goal_id=g.goal_id WHERE t.group_id=$1 AND t.thread_id=$2").bind(group).bind(Uuid::from(thread)).fetch_optional(&mut **tx).await?;
    if let Some(Json(view)) = saved {
        if Uuid::from(view.owner) != node || view.status != GoalStatus::Active {
            return Err(Rejected("only the active goal owner can route its tasks").into());
        }
        if let Input::Submit(request) = input
            && matches!(request.action, RouteAction::Start { .. })
            && !view
                .tasks
                .iter()
                .any(|t| t.dispatching && t.spec.request == *request)
        {
            return Err(Rejected("task dispatch differs from the reserved goal plan").into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/internal/collaboration.rs"]
mod tests;
