//! Local authenticated automation facade and durable dispatch reconciliation.
use crate::{Error, state::Machine};
use orchestrator_coordinator::automations::{ScheduledRoute, Workflow};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Change {
    SaveWorkflow { workflow: Workflow },
    SaveRoute { route: ScheduledRoute },
    Run { workflow_id: Uuid, run_id: Uuid },
}
impl Machine {
    pub(crate) async fn automations(&self) -> Result<Value, Error> {
        let (store, token) = self.chat_store().await?;
        store
            .automations(&token)
            .await
            .map_err(|_| Error::Dependency("automations_unavailable"))
    }
    pub(crate) async fn change_automation(&self, change: Change) -> Result<(), Error> {
        let (store, token) = self.chat_store().await?;
        let result = match change {
            Change::SaveWorkflow { workflow } => store.save_workflow(&token, workflow).await,
            Change::SaveRoute { route } => store.save_route(&token, route).await,
            Change::Run {
                workflow_id,
                run_id,
            } => store.run_workflow(&token, workflow_id, run_id).await,
        };
        result.map_err(|_| Error::Invalid("automation_not_saved_refresh_and_check_inputs"))
    }
    pub(crate) async fn reconcile_automations(&self) -> Result<(), Error> {
        if !self.identity().await?.registration.role.has_router() {
            return Ok(());
        }
        let (store, token) = self.chat_store().await?;
        store
            .schedule_due(&token)
            .await
            .map_err(|_| Error::Dependency("scheduler_unavailable"))?;
        let pending = store
            .pending_workflows(&token)
            .await
            .map_err(|_| Error::Dependency("scheduled_runs_unavailable"))?;
        for request in pending {
            let id = Uuid::from(request.command_id);
            let accepted = self.send_chat(request).await.is_ok();
            store
                .acknowledge_workflow(&token, id, accepted)
                .await
                .map_err(|_| Error::Dependency("run_acknowledgement_unavailable"))?;
        }
        Ok(())
    }
}
