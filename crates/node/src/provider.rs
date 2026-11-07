//! Conservative recovery of human permission replies. No path starts a provider
//! session or replays a command. Only a provider adapter may confirm delivery.

use crate::{NodeError, journal::Journal};
use orchestrator_agents::provider::PermissionProvider;
use orchestrator_protocol::{ids::PermissionId, permission::PermissionState};

/// Observable recovery result, independent of the containing task's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDelivery {
    /// No human decision yet.
    Waiting,
    /// Provider acknowledged this exact decision.
    Delivered,
    /// Delivery was attempted or reconnect is unsupported; never replay blindly.
    NeedsReconciliation,
    /// Request was withdrawn before any response could authorize an effect.
    Abandoned,
}

/// Reconnect and deliver a decided permission at most once. Safe to call after
/// supervisor restart; cancellation after intent commit leaves an unresolved
/// record. A read-only reconnect failure leaves the decision eligible for retry.
///
/// # Errors
/// Reports storage/ownership/expiry failures or provider connection failure.
/// A response failure is reported as `NeedsReconciliation`, not a retryable error.
pub async fn deliver_permission<P: PermissionProvider>(
    journal: &Journal,
    id: PermissionId,
    instance: &str,
    provider: &mut P,
) -> Result<PermissionDelivery, NodeError> {
    let record = journal.permission(id).await?;
    match record.state {
        PermissionState::Abandoned => return Ok(PermissionDelivery::Abandoned),
        PermissionState::Waiting => return Ok(PermissionDelivery::Waiting),
        PermissionState::Delivered => return Ok(PermissionDelivery::Delivered),
        PermissionState::Dispatching => return Ok(PermissionDelivery::NeedsReconciliation),
        PermissionState::Decided => (),
    }
    if !record.request.binding.resumable {
        return Ok(PermissionDelivery::NeedsReconciliation);
    }
    let observed = provider
        .reconnect(&record.request.binding)
        .await
        .map_err(|_| NodeError::Rejected("provider reconnect failed; no permission sent"))?;
    if observed != record.request.binding {
        return Err(NodeError::Rejected(
            "provider reconnected to a different session",
        ));
    }
    let intent = journal.begin_permission_delivery(id, instance).await?;
    if provider.respond(&intent).await.is_err() {
        return Ok(PermissionDelivery::NeedsReconciliation);
    }
    journal.acknowledge_permission(id, instance).await?;
    Ok(PermissionDelivery::Delivered)
}
