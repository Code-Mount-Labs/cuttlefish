//! Local operator review surface. Enrollment requires trusted filesystem access;
//! review credentials are independent of node delivery credentials.

use crate::{NodeError, journal::Journal};
use clap::{Args, Subcommand};
use orchestrator_protocol::{
    ids::{CommandId, PermissionId, ReviewerId},
    permission::{PermissionChoice, PermissionDecision, PermissionRecord},
    sync::NodeToken,
};
use serde::Serialize;
use std::path::PathBuf;

/// Trusted local permission administration and authenticated human review.
#[derive(Debug, Args)]
pub struct ReviewCli {
    #[arg(long)]
    state_dir: PathBuf,
    #[command(subcommand)]
    operation: Operation,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Enroll/rotate a reviewer credential, or disable that reviewer.
    Reviewer {
        #[arg(long)]
        reviewer: ReviewerId,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        disabled: bool,
    },
    /// Read the next page of unresolved requests, including expired requests.
    Pending {
        #[arg(long)]
        after: Option<PermissionId>,
    },
    /// Inspect one request, exact digest and authenticated decision history.
    Inspect {
        #[arg(long)]
        permission: PermissionId,
    },
    /// Record an exact human decision. Omit --allow to deny.
    Decide {
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        permission: PermissionId,
        #[arg(long)]
        decision: CommandId,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        allow: bool,
    },
}

/// Review output carries the exact digest for manual confirmation.
#[derive(Debug, Serialize)]
pub struct ReviewEntry {
    record: PermissionRecord,
    digest: String,
}

impl From<PermissionRecord> for ReviewEntry {
    fn from(record: PermissionRecord) -> Self {
        Self {
            digest: record.request.digest(),
            record,
        }
    }
}

impl ReviewCli {
    /// Execute one local review operation. Empty output means enrollment succeeded.
    ///
    /// # Errors
    /// Reports invalid credential files, unavailable journals and rejected decisions.
    pub async fn run(self) -> Result<Vec<ReviewEntry>, NodeError> {
        let journal = Journal::attach(&self.state_dir.join("node.sqlite")).await?;
        let records = match self.operation {
            Operation::Reviewer {
                reviewer,
                token_file,
                disabled,
            } => {
                journal
                    .configure_reviewer(reviewer, &read_token(token_file)?, !disabled)
                    .await?;
                vec![]
            }
            Operation::Pending { after } => journal.pending_permissions(after).await?,
            Operation::Inspect { permission } => vec![journal.permission(permission).await?],
            Operation::Decide {
                token_file,
                permission,
                decision,
                digest,
                allow,
            } => {
                let input = PermissionDecision {
                    decision_id: decision,
                    permission_id: permission,
                    request_digest: digest,
                    choice: if allow {
                        PermissionChoice::Allow
                    } else {
                        PermissionChoice::Deny
                    },
                };
                vec![
                    journal
                        .decide_permission(&read_token(token_file)?, &input)
                        .await?,
                ]
            }
        };
        Ok(records.into_iter().map(ReviewEntry::from).collect())
    }
}

fn read_token(path: PathBuf) -> Result<NodeToken, NodeError> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(67)
        .read_to_string(&mut text)?;
    if text.len() > 66 {
        return Err(NodeError::Rejected("reviewer token file is too large"));
    }
    NodeToken::try_from(text.trim().to_owned())
        .map_err(|_| NodeError::Rejected("invalid reviewer token file"))
}
