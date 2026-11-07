//! Provider-neutral recovery boundary. Real provider adapters must prove these
//! semantics; the existence of this interface is not proof of provider support.

use orchestrator_protocol::permission::{PermissionRecord, ProviderBinding};
use std::{error::Error, future::Future};

/// Only connection establishment is safely retryable. A permission response
/// may release an external effect and must never be automatically replayed.
pub trait PermissionProvider: Send {
    /// Adapter-specific transport or protocol failure.
    type Error: Error + Send + Sync;

    /// Reconnect to an existing session without starting work or granting permission.
    /// Return the actual identity observed at the provider, for exact comparison.
    ///
    /// # Errors
    /// Must fail if the provider cannot establish the exact original session.
    fn reconnect(
        &mut self,
        binding: &ProviderBinding,
    ) -> impl Future<Output = Result<ProviderBinding, Self::Error>> + Send;

    /// Deliver the persisted human decision for the exact provider callback.
    /// Returning success means the provider acknowledged receipt, not task success.
    ///
    /// # Errors
    /// Every failure is ambiguous: the runtime preserves the intent for reconciliation.
    fn respond(
        &mut self,
        permission: &PermissionRecord,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
