//! Nominal UUID types prevent accidentally interchanging resource identities.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// An identifier was malformed or used the reserved nil UUID.
#[derive(Debug, Error)]
pub enum IdError {
    /// UUID syntax was invalid.
    #[error("invalid UUID: {0}")]
    Invalid(#[from] uuid::Error),
    /// Nil UUIDs are not resource identities.
    #[error("resource identifiers must not be nil")]
    Nil,
}

macro_rules! resource_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "Uuid", into = "Uuid")]
        pub struct $name(Uuid);

        impl TryFrom<Uuid> for $name {
            type Error = IdError;

            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                if value.is_nil() {
                    return Err(IdError::Nil);
                }
                Ok(Self(value))
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)?.try_into()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

resource_id!(GroupId, "A tenant/group identity.");
resource_id!(
    PermissionId,
    "A single immutable runtime permission request."
);
resource_id!(ReviewerId, "An enrolled human permission reviewer.");
resource_id!(GoalId, "A durable collaborative goal identity.");
resource_id!(
    ReleaseId,
    "An immutable capability package or harness revision."
);
resource_id!(NodeId, "A registered computer identity.");
resource_id!(ThreadId, "A conversation identity.");
resource_id!(TaskId, "A logical task identity.");
resource_id!(SessionId, "A persistent specialist session identity.");
resource_id!(CommandId, "A durable command and idempotency identity.");
resource_id!(
    MessageId,
    "A message identity, stable across delivery attempts."
);
resource_id!(
    WorkspaceId,
    "A logical workspace identity, not a filesystem path."
);
