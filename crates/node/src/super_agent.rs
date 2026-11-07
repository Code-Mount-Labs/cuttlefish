//! Durable super-agent decisions; the router only places the resulting work.
use crate::{NodeError, journal::Journal, process::Launcher, runtime::NodeRuntime, sync::Remote};
use orchestrator_agents::planner::{Planner, PlanningContext};
use orchestrator_protocol::{
    capabilities::Capability,
    collaboration::{GoalCommand, GoalStatus, GoalView},
    ids::GoalId,
    sync::NodeProfile,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::types::Json;
use std::{future::Future, time::Duration};

impl Journal {
    /// Save one exact decision before transport; never replace an unacknowledged decision.
    ///
    /// # Errors
    /// Rejects conflicting pending operations and unavailable storage.
    pub async fn stage_goal(&self, goal: GoalId, command: &GoalCommand) -> Result<(), NodeError> {
        let Json(saved):Json<GoalCommand>=sqlx::query_scalar("INSERT INTO super_agent_outbox(goal_id,command) VALUES (?,?) ON CONFLICT(goal_id) DO UPDATE SET command=super_agent_outbox.command RETURNING command").bind(goal.to_string()).bind(Json(command)).fetch_one(&self.pool).await?;
        if saved != *command {
            return Err(NodeError::Rejected(
                "another goal decision is awaiting acknowledgment",
            ));
        }
        Ok(())
    }
    /// Recover the exact pending decision after supervisor replacement.
    ///
    /// # Errors
    /// Reports corrupt or unavailable local storage.
    pub async fn pending_goal(&self, goal: GoalId) -> Result<Option<GoalCommand>, NodeError> {
        let result: Option<Json<GoalCommand>> =
            sqlx::query_scalar("SELECT command FROM super_agent_outbox WHERE goal_id=?")
                .bind(goal.to_string())
                .fetch_optional(&self.pool)
                .await?;
        Ok(result.map(|c| c.0))
    }
    /// Retire a decision after acknowledgment or a definitive conflict response.
    ///
    /// # Errors
    /// Reports unavailable local storage.
    pub async fn clear_goal(&self, goal: GoalId) -> Result<(), NodeError> {
        sqlx::query("DELETE FROM super_agent_outbox WHERE goal_id=?")
            .bind(goal.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Reserve a model turn durably, including failed calls. Restarts do not reset the limit.
    ///
    /// # Errors
    /// Rejects exhausted or zero budgets and unavailable storage.
    pub async fn reserve_turn(&self, goal: GoalId, limit: u32) -> Result<(), NodeError> {
        if limit == 0 {
            return Err(NodeError::Rejected("planning budget must be positive"));
        }
        let result=sqlx::query("INSERT INTO planning_budget(goal_id,turns) VALUES (?,1) ON CONFLICT(goal_id) DO UPDATE SET turns=turns+1 WHERE turns<?").bind(goal.to_string()).bind(limit).execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(NodeError::Rejected("planning budget exhausted"));
        }
        Ok(())
    }
}
impl Remote {
    async fn exchange(
        &self,
        path: &str,
        method: Method,
        body: Option<Value>,
    ) -> Result<Value, NodeError> {
        let mut endpoint = self.endpoint.clone();
        endpoint.set_path(&format!(
            "{}{path}",
            self.endpoint.path().trim_end_matches("/v1/sync")
        ));
        let mut request = self
            .client
            .request(method, endpoint)
            .bearer_auth(self.token.expose());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request.send().await?.error_for_status()?;
        if !response.status().is_success() {
            return Err(NodeError::Rejected("unexpected collaboration HTTP status"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 4_194_304 {
                return Err(NodeError::Rejected("collaboration response exceeds 4 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    /// Execute one crash-safe coordination step. Specialists remain independent processes.
    ///
    /// # Errors
    /// Reports transport, journal, budget or rejected planning decisions.
    pub async fn super_step<L: Launcher, P: Planner>(
        &self,
        runtime: &mut NodeRuntime<L>,
        profile: &NodeProfile,
        goal: GoalId,
        planner: &P,
        turn_limit: u32,
    ) -> Result<(), NodeError> {
        let mut profile = profile.clone();
        profile.capabilities.extend(Capability::native());
        self.cycle(runtime, &profile).await?;
        let journal = runtime.journal();
        if let Some(command) = journal.pending_goal(goal).await? {
            return self.flush_goal(journal, goal, &command).await;
        }
        let mut view = self.goal_view(journal, goal).await?;
        if view.status != GoalStatus::Active {
            return Ok(());
        }
        if let Some(task) = view
            .tasks
            .iter()
            .find(|t| t.dispatching && t.execution.is_none())
        {
            let task = orchestrator_domain::collaboration::task_id(&task.spec)?;
            match self
                .exchange(
                    &format!("/v1/goals/{goal}/tasks/{task}/dispatch"),
                    Method::POST,
                    None,
                )
                .await
            {
                Ok(_) => {
                    view = self.goal_view(journal, goal).await?;
                }
                Err(NodeError::Transport(error))
                    if error.status() == Some(StatusCode::SERVICE_UNAVAILABLE) => {}
                Err(error) => return Err(error),
            }
        }
        let context = PlanningContext {
            installations: serde_json::from_value(
                self.exchange("/v1/capabilities/installations", Method::GET, None)
                    .await?,
            )?,
            goal: view,
            peers: serde_json::from_value(self.exchange("/v1/nodes", Method::GET, None).await?)?,
            catalog: serde_json::from_value(
                self.exchange("/v1/capabilities", Method::GET, None).await?,
            )?,
            native: Capability::native(),
            workspaces: journal.dispatch_poll().await?.workspaces,
        };
        journal.reserve_turn(goal, turn_limit).await?;
        if let Some(command) = planner.decide(&context).await? {
            orchestrator_domain::collaboration::apply(
                &context.goal,
                journal.identity().node_id,
                &command,
                &context.catalog,
            )?;
            journal.stage_goal(goal, &command).await?;
            self.flush_goal(journal, goal, &command).await?;
        }
        Ok(())
    }
    async fn goal_view(&self, journal: &Journal, goal: GoalId) -> Result<GoalView, NodeError> {
        let view: GoalView = serde_json::from_value(
            self.exchange(&format!("/v1/goals/{goal}"), Method::GET, None)
                .await?,
        )?;
        if view.spec.goal_id != goal || view.owner != journal.identity().node_id {
            return Err(NodeError::Rejected(
                "goal response has the wrong identity or owner",
            ));
        }
        Ok(view)
    }

    async fn flush_goal(
        &self,
        journal: &Journal,
        goal: GoalId,
        command: &GoalCommand,
    ) -> Result<(), NodeError> {
        match self
            .exchange(
                &format!("/v1/goals/{goal}/commands"),
                Method::POST,
                Some(json!(command)),
            )
            .await
        {
            Ok(value) => {
                let view: GoalView = serde_json::from_value(value)?;
                if view.spec.goal_id != goal
                    || view.owner != journal.identity().node_id
                    || command.expected_revision.checked_add(1) != Some(view.revision)
                {
                    return Err(NodeError::Rejected("invalid goal acknowledgment"));
                }
                journal.clear_goal(goal).await
            }
            Err(NodeError::Transport(error)) if error.status() == Some(StatusCode::CONFLICT) => {
                journal.clear_goal(goal).await?;
                Err(error.into())
            }
            Err(error) => Err(error),
        }
    }
    /// Run the owner loop with bounded, journaled planning turns.
    ///
    /// # Errors
    /// Stops on a failed step or shutdown signal so an operator can inspect it.
    pub async fn supervise<L: Launcher, P: Planner>(
        &self,
        runtime: &mut NodeRuntime<L>,
        profile: &NodeProfile,
        goal: GoalId,
        planner: &P,
        turn_limit: u32,
        shutdown: impl Future<Output = std::io::Result<()>>,
    ) -> Result<(), NodeError> {
        tokio::pin!(shutdown);
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {result=&mut shutdown=>return result.map_err(NodeError::from),_=interval.tick()=>{self.super_step(runtime,profile,goal,planner,turn_limit).await?;}}
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/super_agent.rs"]
mod tests;
