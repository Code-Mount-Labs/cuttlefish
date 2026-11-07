//! Two-phase routing: prepare context, ask a model, then revalidate and commit.
use crate::{
    Error,
    dispatch::{Input, Prepared},
    selector::Selector,
    store::Store,
};
use orchestrator_protocol::{
    ids::ThreadId,
    routing::{DispatchView, RouteRequest},
    sync::NodeToken,
};
use std::sync::Arc;

/// Routing has only selection and persistence powers; execution lives on nodes.
#[derive(Debug, Clone)]
pub struct RoutingService {
    store: Store,
    selector: Arc<dyn Selector>,
}
impl RoutingService {
    /// Dispatch a reserved goal task using the normal affinity and failover rules.
    ///
    /// # Errors
    /// Rejects unauthorized goals, unreserved work and unavailable destinations.
    pub async fn delegate(
        &self,
        token: &NodeToken,
        goal: orchestrator_protocol::ids::GoalId,
        task: orchestrator_protocol::ids::TaskId,
    ) -> Result<DispatchView, Error> {
        let request = self.store.delegation(token, goal, task).await?;
        self.submit(token, request).await
    }
    /// Supply shared storage and a constrained selector.
    pub fn new(store: Store, selector: Arc<dyn Selector>) -> Self {
        Self { store, selector }
    }
    /// Route a new request or return its original immutable receipt on retry.
    ///
    /// # Errors
    /// Rejects conflicting requests, invalid choices, stale context and unavailable nodes.
    pub async fn submit(
        &self,
        token: &NodeToken,
        request: RouteRequest,
    ) -> Result<DispatchView, Error> {
        self.execute(token, Input::Submit(request)).await
    }
    /// Move only a provably unclaimed start away from an unavailable node.
    ///
    /// # Errors
    /// Refuses replay of claimed work or migration of an existing specialist session.
    pub async fn failover(
        &self,
        token: &NodeToken,
        thread: ThreadId,
    ) -> Result<DispatchView, Error> {
        self.execute(token, Input::Failover(thread)).await
    }
    async fn execute(&self, token: &NodeToken, input: Input) -> Result<DispatchView, Error> {
        match self.store.prepare(token, input).await? {
            Prepared::Ready(view) => Ok(*view),
            Prepared::Choose(plan) => {
                let node = if let Some(node) = plan.affinity {
                    node
                } else {
                    self.selector.choose(&plan.context).await?
                };
                self.store.commit_route(token, *plan, node).await
            }
        }
    }
}
