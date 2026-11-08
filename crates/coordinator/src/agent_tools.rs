//! Provider collaboration is scoped to the authenticated executing parent.
use crate::{Error, routing::RoutingService, store::Store};
use orchestrator_domain::collaboration as policy;
use orchestrator_protocol::{
    agent::Message,
    collaboration::{GoalAction, GoalCommand, GoalSpec, TaskSpec},
    execution::ProviderWorkspace,
    ids::{GoalId, NodeId},
    routing::{RouteAction, RouteRequest},
    sync::NodeToken,
    tools::{ToolAction, ToolRequest},
};
use serde_json::{Value, json};
use sqlx::types::Json;
use uuid::Uuid;
impl RoutingService {
    /// Execute a typed collaboration tool using the current parent as its scope.
    ///
    /// # Errors
    /// Rejects stale parents, cross-group access, privilege escalation and invalid plans.
    pub async fn agent_tool(
        &self,
        token: &NodeToken,
        request: &ToolRequest,
    ) -> Result<Value, Error> {
        let (thread, goal) = self.store.tool_scope(token, request).await?;
        match &request.action {
            ToolAction::Agents {} => Ok(
                json!({"agents":self.store.nodes(token).await?,"workspaces":self.store.provider_workspaces(token).await?}),
            ),
            ToolAction::Delegate { .. } => {
                let task = self
                    .store
                    .prepare_child(token, request, thread, goal)
                    .await?;
                let receipt = self.delegate(token, goal, task).await?;
                Ok(
                    json!({"task_id":task,"thread_id":receipt.request.thread_id,"node_id":receipt.command.node_id,"state":receipt.state}),
                )
            }
            ToolAction::Task { task_id } => {
                let view = self.store.goal(token, goal).await?;
                let task = policy::find(&view, *task_id)?;
                Ok(json!({
                    "task_id":task_id,
                    "state":task.execution.as_ref().map(|execution|execution.state),
                    "node_id":task.execution.as_ref().map(|execution|execution.command.node_id),
                    "progress":task.progress,
                    "outcome":task.execution.as_ref().and_then(|execution|execution.outcome.as_ref()),
                }))
            }
        }
    }
}
impl Store {
    async fn tool_scope(
        &self,
        token: &NodeToken,
        request: &ToolRequest,
    ) -> Result<(orchestrator_protocol::ids::ThreadId, GoalId), Error> {
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        let thread: Option<Json<orchestrator_protocol::ids::ThreadId>> = sqlx::query_scalar("SELECT to_jsonb(thread_id) FROM dispatches WHERE group_id=$1 AND node_id=$2 AND command_id=$3 AND state='claimed'")
            .bind(group).bind(node).bind(Uuid::from(request.parent)).fetch_optional(&mut *tx).await?;
        let thread = thread.ok_or(Error::RoutingConflict(
            "Collaboration requires an executing parent task.",
        ))?;
        Ok((thread.0, thread.0.collaboration_goal()))
    }
    async fn prepare_child(
        &self,
        token: &NodeToken,
        request: &ToolRequest,
        thread: orchestrator_protocol::ids::ThreadId,
        goal: GoalId,
    ) -> Result<orchestrator_protocol::ids::TaskId, Error> {
        let ToolAction::Delegate {
            workspace_id,
            message,
            acceptance,
        } = &request.action
        else {
            return Err(Error::RoutingConflict("A delegation request is required."));
        };
        let (mut tx, group, node) = self.routing_transaction(token).await?;
        // Recheck under the group lock: a completed parent cannot create new work.
        let source: Option<(Json<ProviderWorkspace>,String,Json<NodeId>)> = sqlx::query_as("SELECT w.workspace,d.request#>>'{action,message,text}',to_jsonb(d.node_id) FROM dispatches d JOIN thread_bindings b ON b.group_id=d.group_id AND b.thread_id=d.thread_id JOIN provider_workspaces w ON w.group_id=d.group_id AND w.workspace_id=(b.binding->>'workspace_id')::uuid WHERE d.group_id=$1 AND d.node_id=$2 AND d.command_id=$3 AND d.state='claimed' AND NOT EXISTS(SELECT 1 FROM goal_tasks t WHERE t.group_id=d.group_id AND t.thread_id=d.thread_id)")
            .bind(group).bind(node).bind(Uuid::from(request.parent)).fetch_optional(&mut *tx).await?;
        let (source, objective, Json(owner)) = source.ok_or(Error::RoutingConflict("Only an executing top-level agent may delegate. Child agents must return results to their parent."))?;
        let destination: Option<Json<ProviderWorkspace>> = sqlx::query_scalar(
            "SELECT workspace FROM provider_workspaces WHERE group_id=$1 AND workspace_id=$2",
        )
        .bind(group)
        .bind(Uuid::from(*workspace_id))
        .fetch_optional(&mut *tx)
        .await?;
        let destination = destination
            .ok_or(Error::RoutingConflict(
                "Select a registered workspace in this group.",
            ))?
            .0;
        if !source.allow_edits && destination.allow_edits {
            return Err(Error::RoutingConflict(
                "A read-only parent cannot delegate to a workspace with edit permissions.",
            ));
        }
        let spec = GoalSpec {
            goal_id: goal,
            thread_id: thread,
            objective: objective
                .try_into()
                .map_err(|_| Error::RoutingConflict("Parent message is missing."))?,
            acceptance: acceptance.clone(),
        };
        let initial = policy::create(spec, owner);
        sqlx::query("INSERT INTO goals(group_id,goal_id,thread_id,owner_node,snapshot) VALUES($1,$2,$3,$4,$5) ON CONFLICT(group_id,goal_id) DO NOTHING")
            .bind(group).bind(Uuid::from(goal)).bind(Uuid::from(thread)).bind(node).bind(Json(initial)).execute(&mut *tx).await?;
        let mut view = crate::collaboration::load(&mut tx, group, goal).await?;
        if view.owner != owner || view.spec.thread_id != thread {
            return Err(Error::Unauthorized);
        }
        let id = Uuid::from(request.id);
        let spec = child_spec(
            request.id,
            *workspace_id,
            destination.provider,
            message.clone(),
            acceptance.clone(),
        );
        let task_id = request.id.delegation().task;
        if let Some(saved) = view
            .tasks
            .iter()
            .find(|task| task.spec.request.command_id == request.id)
        {
            if saved.spec != spec {
                return Err(Error::RoutingConflict(
                    "Delegation identity conflicts with its saved request.",
                ));
            }
        } else {
            view = policy::apply(
                &view,
                owner,
                &GoalCommand {
                    command_id: request.id,
                    expected_revision: view.revision,
                    action: GoalAction::Plan { tasks: vec![spec] },
                },
                &[],
            )?;
            view = policy::apply(
                &view,
                owner,
                &GoalCommand {
                    command_id: request.id,
                    expected_revision: view.revision,
                    action: GoalAction::Delegate { task_id },
                },
                &[],
            )?;
            sqlx::query("INSERT INTO goal_tasks(group_id,goal_id,task_id,thread_id,command_id) VALUES($1,$2,$3,$3,$3)")
                .bind(group).bind(Uuid::from(goal)).bind(id).execute(&mut *tx).await?;
            crate::collaboration::save(&mut tx, group, &view).await?;
        }
        tx.commit().await?;
        Ok(task_id)
    }
}

fn child_spec(
    id: orchestrator_protocol::ids::CommandId,
    workspace_id: orchestrator_protocol::ids::WorkspaceId,
    provider: orchestrator_protocol::execution::LocalProvider,
    message: orchestrator_protocol::values::MessageText,
    acceptance: orchestrator_protocol::values::MessageText,
) -> TaskSpec {
    let ids = id.delegation();
    TaskSpec {
        request: RouteRequest {
            command_id: id,
            thread_id: ids.thread,
            action: RouteAction::Start {
                task_id: ids.task,
                session_id: ids.session,
                workspace_id,
                capabilities: [
                    orchestrator_protocol::capabilities::Capability::local_provider(provider),
                ]
                .into(),
                message: Message {
                    id: ids.message,
                    text: message,
                    attachments: vec![],
                },
            },
        },
        dependencies: std::collections::BTreeSet::new(),
        acceptance,
    }
}

#[cfg(test)]
#[path = "../tests/internal/agent_tools.rs"]
mod tests;
