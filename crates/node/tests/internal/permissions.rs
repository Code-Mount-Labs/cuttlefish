#![allow(clippy::expect_used)]

use super::*;
use crate::test_support as common;
use crate::{
    model::CommandStatus,
    provider::{PermissionDelivery, deliver_permission},
};
use common::{Fixture, id, instance, start};
#[path = "permission_faults.rs"]
mod faults;
use orchestrator_agents::provider::PermissionProvider;
use orchestrator_protocol::permission::PermissionChoice;

pub(crate) fn request() -> PermissionRequest {
    PermissionRequest {
        permission_id: id(200),
        command_id: id(6),
        binding: ProviderBinding {
            group_id: id(1),
            node_id: id(2),
            target: start().target,
            provider: "coding.test".to_owned().try_into().expect("provider"),
            provider_session_id: "external-session".to_owned().try_into().expect("session"),
            resumable: true,
        },
        provider_request_id: "tool-call-1".to_owned().try_into().expect("request"),
        action: r#"{"tool":"write","path":"src/main.rs","content":"fn main() {}"}"#
            .to_owned()
            .try_into()
            .expect("action"),
        expires_at_ms: 4_000_000_000_000,
    }
}
fn token() -> NodeToken {
    "b".repeat(64).try_into().expect("review token")
}
fn decision(request: &PermissionRequest) -> PermissionDecision {
    PermissionDecision {
        decision_id: id(201),
        permission_id: request.permission_id,
        request_digest: request.digest(),
        choice: PermissionChoice::Allow,
    }
}
pub(crate) async fn setup() -> Fixture {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim")
        .expect("command");
    f.journal
        .bind_provider(id(6), &instance().to_string(), &request().binding)
        .await
        .expect("bind");
    f.journal
        .configure_reviewer(id(202), &token(), true)
        .await
        .expect("reviewer");
    f
}
pub(crate) async fn pending(f: &Fixture) {
    f.journal
        .request_permission(&instance().to_string(), &request())
        .await
        .expect("request");
}
async fn approved(f: &Fixture) {
    pending(f).await;
    f.journal
        .decide_permission(&token(), &decision(&request()))
        .await
        .expect("approve");
}
async fn sql(f: &Fixture, statement: &'static str) {
    let pool = f.raw().await;
    sqlx::query(statement)
        .execute(&pool)
        .await
        .expect("test SQL");
    pool.close().await;
}

#[derive(Default)]
struct Provider {
    reconnect_error: bool,
    wrong_session: bool,
    response_error: bool,
    reconnects: usize,
    replies: Vec<PermissionRecord>,
    break_ack: Option<sqlx::SqlitePool>,
}
impl PermissionProvider for Provider {
    type Error = std::io::Error;
    async fn reconnect(
        &mut self,
        binding: &ProviderBinding,
    ) -> Result<ProviderBinding, Self::Error> {
        self.reconnects += 1;
        if self.reconnect_error {
            return Err(std::io::Error::other("disconnected"));
        }
        let mut result = binding.clone();
        if self.wrong_session {
            result.provider_session_id = "new-session".to_owned().try_into().expect("session");
        }
        Ok(result)
    }
    async fn respond(&mut self, record: &PermissionRecord) -> Result<(), Self::Error> {
        self.replies.push(record.clone());
        if let Some(pool) = &self.break_ack {
            sqlx::query("CREATE TRIGGER fail_ack BEFORE UPDATE ON runtime_permissions BEGIN SELECT RAISE(ABORT,'lost acknowledgement'); END")
                .execute(pool).await.expect("fault");
        }
        if self.response_error {
            return Err(std::io::Error::other("effect happened; response lost"));
        }
        Ok(())
    }
}
async fn deliver(
    journal: &Journal,
    provider: &mut Provider,
) -> Result<PermissionDelivery, NodeError> {
    deliver_permission(journal, id(200), &instance().to_string(), provider).await
}

#[tokio::test]
async fn waiting_and_approved_permissions_survive_reopening_without_reissuing_work() {
    let f = setup().await;
    pending(&f).await;
    let saved = f.journal.permission(id(200)).await.expect("saved");
    let mut provider = Provider::default();
    assert_eq!(
        deliver(&f.journal, &mut provider).await.expect("waiting"),
        PermissionDelivery::Waiting
    );
    assert_eq!(provider.reconnects, 0);
    let path = f.journal.path().to_owned();
    f.journal.close().await;
    let reopened = Journal::open(&path, common::identity())
        .await
        .expect("restart");
    assert_eq!(
        reopened
            .permission(id(200))
            .await
            .expect("pending restored"),
        saved
    );
    assert_eq!(
        reopened.provider_binding(id(4)).await.expect("binding"),
        Some(request().binding)
    );
    let approved = reopened
        .decide_permission(&token(), &decision(&request()))
        .await
        .expect("approve after restart");
    assert_eq!(
        approved.review.as_ref().expect("review").reviewer_id,
        id(202)
    );
    assert!(approved.review.as_ref().expect("review").decided_at_ms > 0);
    reopened.close().await;
    let reopened = Journal::attach(&path).await.expect("second restart");
    assert_eq!(
        reopened
            .permission(id(200))
            .await
            .expect("approval restored"),
        approved
    );
    assert_eq!(
        deliver(&reopened, &mut provider).await.expect("deliver"),
        PermissionDelivery::Delivered
    );
    assert_eq!(
        deliver(&reopened, &mut provider).await.expect("duplicate"),
        PermissionDelivery::Delivered
    );
    assert_eq!(provider.reconnects, 1);
    assert_eq!(provider.replies.len(), 1);
    assert_eq!(provider.replies[0].request, request());
    assert_eq!(provider.replies[0].review, approved.review);
    assert_eq!(
        reopened
            .command(id(6))
            .await
            .expect("command")
            .expect("exists")
            .status,
        CommandStatus::Executing
    );
    reopened
        .acknowledge_permission(id(200), &instance().to_string())
        .await
        .expect("duplicate ack");
    reopened
        .complete(id(6), &instance().to_string(), &common::outcome(&start()))
        .await
        .expect("separate completion");
    reopened.close().await;
}

#[tokio::test]
async fn ambiguous_delivery_and_lost_local_ack_are_never_replayed_after_restart() {
    for lost_ack in [false, true] {
        let f = setup().await;
        approved(&f).await;
        let mut provider = Provider {
            response_error: !lost_ack,
            break_ack: if lost_ack { Some(f.raw().await) } else { None },
            ..Provider::default()
        };
        let result = deliver(&f.journal, &mut provider).await;
        if lost_ack {
            assert!(result.is_err());
        } else {
            assert_eq!(
                result.expect("ambiguous"),
                PermissionDelivery::NeedsReconciliation
            );
        }
        assert_eq!(provider.replies.len(), 1);
        assert_eq!(
            f.journal.permission(id(200)).await.expect("intent").state,
            PermissionState::Dispatching
        );
        let path = f.journal.path().to_owned();
        f.journal.close().await;
        let reopened = Journal::attach(&path).await.expect("restart");
        let mut replacement = Provider::default();
        assert_eq!(
            deliver(&reopened, &mut replacement)
                .await
                .expect("recovery"),
            PermissionDelivery::NeedsReconciliation
        );
        assert_eq!(replacement.reconnects, 0);
        assert!(replacement.replies.is_empty());
        assert!(
            reopened
                .complete(id(6), &instance().to_string(), &common::outcome(&start()))
                .await
                .is_err()
        );
        reopened.close().await;
    }
}

#[tokio::test]
async fn safe_connection_retries_do_not_change_provider_identity_or_decision() {
    let f = setup().await;
    approved(&f).await;
    for (reconnect_error, wrong_session) in [(true, false), (false, true)] {
        let mut provider = Provider {
            reconnect_error,
            wrong_session,
            ..Provider::default()
        };
        assert!(deliver(&f.journal, &mut provider).await.is_err());
        assert!(provider.replies.is_empty());
        assert_eq!(
            f.journal.permission(id(200)).await.expect("record").state,
            PermissionState::Decided
        );
    }
    f.journal
        .configure_reviewer(id(202), &token(), false)
        .await
        .expect("revoke");
    assert!(deliver(&f.journal, &mut Provider::default()).await.is_err());
    f.journal
        .configure_reviewer(id(202), &token(), true)
        .await
        .expect("enable");
    assert_eq!(
        deliver(&f.journal, &mut Provider::default())
            .await
            .expect("recover"),
        PermissionDelivery::Delivered
    );
    f.journal.close().await;
}

#[tokio::test]
async fn bindings_and_callbacks_are_immutable_and_scoped_to_original_worker() {
    let f = setup().await;
    let original = request();
    f.journal
        .bind_provider(id(6), &instance().to_string(), &original.binding)
        .await
        .expect("same binding");
    assert_eq!(
        f.journal.provider_binding(id(99)).await.expect("unknown"),
        None
    );
    assert!(
        f.journal
            .bind_provider(id(6), "wrong", &original.binding)
            .await
            .is_err()
    );
    for field in 0..6 {
        let mut altered = original.clone();
        match field {
            0 => altered.binding.group_id = id(91),
            1 => altered.binding.node_id = id(91),
            2 => altered.binding.target.execution_epoch = 2.try_into().expect("epoch"),
            3 => {
                altered.binding.provider_session_id =
                    "other".to_owned().try_into().expect("session");
            }
            4 => altered.binding.resumable = false,
            _ => altered.binding.provider = "other".to_owned().try_into().expect("provider"),
        }
        assert!(
            f.journal
                .bind_provider(id(6), &instance().to_string(), &altered.binding)
                .await
                .is_err()
        );
        assert!(
            f.journal
                .request_permission(&instance().to_string(), &altered)
                .await
                .is_err()
        );
    }
    pending(&f).await;
    let saved = f.journal.permission(id(200)).await.expect("record");
    assert_eq!(
        f.journal
            .request_permission(&instance().to_string(), &original)
            .await
            .expect("retry"),
        saved
    );
    let mut altered = original.clone();
    altered.action = "write different bytes"
        .to_owned()
        .try_into()
        .expect("action");
    assert!(
        f.journal
            .request_permission(&instance().to_string(), &altered)
            .await
            .is_err()
    );
    assert!(
        f.journal
            .request_permission("wrong", &original)
            .await
            .is_err()
    );
    assert!(
        f.journal
            .complete(id(6), &instance().to_string(), &common::outcome(&start()))
            .await
            .is_err()
    );
    assert!(
        f.journal
            .acknowledge_permission(id(200), &instance().to_string())
            .await
            .is_err()
    );
    assert!(
        f.journal
            .begin_permission_delivery(id(200), &instance().to_string())
            .await
            .is_err()
    );
    f.journal.close().await;
}

#[tokio::test]
async fn only_authenticated_exact_once_decisions_are_accepted_including_denials() {
    let f = setup().await;
    pending(&f).await;
    let mut input = decision(&request());
    let wrong: NodeToken = "a".repeat(64).try_into().expect("node token");
    assert!(f.journal.decide_permission(&wrong, &input).await.is_err());
    input.request_digest = "incorrect".to_owned();
    assert!(f.journal.decide_permission(&token(), &input).await.is_err());
    input = decision(&request());
    input.choice = PermissionChoice::Deny;
    let saved = f
        .journal
        .decide_permission(&token(), &input)
        .await
        .expect("deny");
    assert_eq!(
        f.journal
            .decide_permission(&token(), &input)
            .await
            .expect("duplicate"),
        saved
    );
    assert!(
        f.journal
            .decide_permission(&token(), &decision(&request()))
            .await
            .is_err()
    );
    f.journal
        .configure_reviewer(id(203), &wrong, true)
        .await
        .expect("second reviewer");
    assert!(f.journal.decide_permission(&wrong, &input).await.is_err());
    let mut provider = Provider::default();
    assert_eq!(
        deliver(&f.journal, &mut provider)
            .await
            .expect("denial delivered"),
        PermissionDelivery::Delivered
    );
    assert_eq!(
        provider.replies[0]
            .review
            .as_ref()
            .expect("review")
            .decision
            .choice,
        PermissionChoice::Deny
    );
    assert!(
        f.journal
            .pending_permissions(None)
            .await
            .expect("pending")
            .is_empty()
    );
    f.journal.close().await;
}

#[tokio::test]
async fn concurrent_review_and_delivery_have_one_winner() {
    let f = setup().await;
    pending(&f).await;
    let allow = decision(&request());
    let deny = PermissionDecision {
        decision_id: id(210),
        choice: PermissionChoice::Deny,
        ..allow.clone()
    };
    let credential = token();
    let (a, b) = tokio::join!(
        f.journal.decide_permission(&credential, &allow),
        f.journal.decide_permission(&credential, &deny)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let worker = instance().to_string();
    let (a, b) = tokio::join!(
        f.journal.begin_permission_delivery(id(200), &worker),
        f.journal.begin_permission_delivery(id(200), &worker)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        f.journal.permission(id(200)).await.expect("intent").state,
        PermissionState::Dispatching
    );
    f.journal.close().await;
}

#[tokio::test]
async fn expiration_is_rechecked_and_pending_pages_keep_expired_work_visible() {
    let f = setup().await;
    let mut expired = request();
    expired.expires_at_ms = 1;
    assert!(
        f.journal
            .request_permission(&instance().to_string(), &expired)
            .await
            .is_err()
    );
    pending(&f).await;
    sql(
        &f,
        "UPDATE runtime_permissions SET record=json_set(record,'$.request.expires_at_ms',1)",
    )
    .await;
    assert!(
        f.journal
            .decide_permission(&token(), &decision(&expired))
            .await
            .is_err()
    );
    assert_eq!(
        f.journal
            .pending_permissions(None)
            .await
            .expect("list")
            .len(),
        1
    );
    assert!(
        f.journal
            .pending_permissions(Some(id(200)))
            .await
            .expect("next page")
            .is_empty()
    );
    sql(&f, "UPDATE runtime_permissions SET record=json_set(record,'$.request.expires_at_ms',4000000000000)").await;
    f.journal
        .decide_permission(&token(), &decision(&request()))
        .await
        .expect("approve");
    // Preserve a matching scope digest while forcing the expiry boundary in stored test data.
    let mut record = f.journal.permission(id(200)).await.expect("record");
    record.request.expires_at_ms = 1;
    record
        .review
        .as_mut()
        .expect("review")
        .decision
        .request_digest = record.request.digest();
    let mut connection = f.journal.pool.acquire().await.expect("connection");
    save(&mut connection, &record).await.expect("test expiry");
    drop(connection);
    assert!(
        f.journal
            .begin_permission_delivery(id(200), &instance().to_string())
            .await
            .is_err()
    );
    f.journal.close().await;
}

#[tokio::test]
async fn supervisor_replacement_preserves_waits_and_fences_stale_workers() {
    use crate::{process::LocalLauncher, runtime::NodeRuntime};
    let f = setup().await;
    pending(&f).await;
    let open = || {
        NodeRuntime::open(
            f.temp.path(),
            common::identity(),
            LocalLauncher::new("/no-relaunch-permitted".into()),
        )
    };
    let supervisor = open().await.expect("first supervisor");
    supervisor.journal().close().await;
    drop(supervisor);
    let mut replacement = open().await.expect("replacement supervisor");
    assert_eq!(
        replacement
            .submit(&start())
            .await
            .expect("redelivery")
            .status,
        CommandStatus::Executing
    );
    assert_eq!(
        replacement
            .journal()
            .pending_permissions(None)
            .await
            .expect("pending")
            .len(),
        1
    );
    f.journal
        .decide_permission(&token(), &decision(&request()))
        .await
        .expect("approve");
    assert!(
        f.journal
            .begin_permission_delivery(id(200), "stale-instance")
            .await
            .is_err()
    );
    f.journal
        .interrupted(id(4), &instance().to_string(), "connection lost")
        .await
        .expect("interrupted");
    assert!(
        f.journal
            .begin_permission_delivery(id(200), &instance().to_string())
            .await
            .is_err()
    );
    assert!(
        f.journal
            .claim(id(4), &instance().to_string())
            .await
            .expect("no replay")
            .is_none()
    );
    f.journal
        .heartbeat(id(4), &instance().to_string())
        .await
        .expect("original worker returns");
    assert_eq!(
        deliver(replacement.journal(), &mut Provider::default())
            .await
            .expect("resume exact permission"),
        PermissionDelivery::Delivered
    );
    replacement.journal().close().await;
    f.journal.close().await;
}

#[tokio::test]
async fn unsupported_resume_stays_unresolved_and_withdrawal_requires_new_approval() {
    let f = Fixture::new().await;
    f.registered().await;
    f.journal
        .claim(id(4), &instance().to_string())
        .await
        .expect("claim");
    let mut input = request();
    input.binding.resumable = false;
    f.journal
        .bind_provider(id(6), &instance().to_string(), &input.binding)
        .await
        .expect("bind");
    f.journal
        .configure_reviewer(id(202), &token(), true)
        .await
        .expect("reviewer");
    f.journal
        .request_permission(&instance().to_string(), &input)
        .await
        .expect("request");
    f.journal
        .decide_permission(&token(), &decision(&input))
        .await
        .expect("approve");
    let mut provider = Provider::default();
    assert_eq!(
        deliver(&f.journal, &mut provider)
            .await
            .expect("unsupported"),
        PermissionDelivery::NeedsReconciliation
    );
    assert_eq!(provider.reconnects, 0);
    for _ in 0..2 {
        f.journal
            .abandon_permission(id(200), &instance().to_string())
            .await
            .expect("withdraw");
    }
    assert_eq!(
        deliver(&f.journal, &mut provider).await.expect("withdrawn"),
        PermissionDelivery::Abandoned
    );
    assert!(
        f.journal
            .decide_permission(&token(), &decision(&input))
            .await
            .is_err()
    );
    input.permission_id = id(220);
    f.journal
        .request_permission(&instance().to_string(), &input)
        .await
        .expect("new request for same callback");
    assert!(
        f.journal
            .begin_permission_delivery(id(220), &instance().to_string())
            .await
            .is_err()
    );
    f.journal
        .abandon_permission(id(220), &instance().to_string())
        .await
        .expect("withdraw undecided");
    f.journal
        .complete(id(6), &instance().to_string(), &common::outcome(&start()))
        .await
        .expect("completion after choosing another approach");
    f.journal.close().await;
}

#[tokio::test]
async fn tampered_evidence_and_attempted_delivery_cannot_be_cleared_or_reapproved() {
    let f = setup().await;
    approved(&f).await;
    sql(&f, "UPDATE runtime_permissions SET record=json_set(record,'$.review.decision.request_digest','wrong')").await;
    assert!(
        f.journal
            .begin_permission_delivery(id(200), &instance().to_string())
            .await
            .is_err()
    );
    let mut record = f.journal.permission(id(200)).await.expect("record");
    record
        .review
        .as_mut()
        .expect("review")
        .decision
        .request_digest = request().digest();
    let mut conn = f.journal.pool.acquire().await.expect("connection");
    save(&mut conn, &record).await.expect("restore");
    drop(conn);
    f.journal
        .begin_permission_delivery(id(200), &instance().to_string())
        .await
        .expect("intent");
    assert!(
        f.journal
            .abandon_permission(id(200), &instance().to_string())
            .await
            .is_err()
    );
    assert!(
        f.journal
            .acknowledge_permission(id(200), "wrong")
            .await
            .is_err()
    );
    f.journal
        .acknowledge_permission(id(200), &instance().to_string())
        .await
        .expect("provider confirmed");
    assert!(
        f.journal
            .abandon_permission(id(200), &instance().to_string())
            .await
            .is_err()
    );
    f.journal.close().await;
}
