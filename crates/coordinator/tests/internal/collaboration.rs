#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{common::Fixture, data};

#[tokio::test]
async fn internal_transaction_failures_do_not_become_approvals() {
    let f = Fixture::enrolled().await;
    let mut tx = f.raw.begin().await.expect("transaction");
    assert!(sqlx::query("SELECT 1/0").execute(&mut *tx).await.is_err());
    assert!(
        validate_nodes(
            &mut tx,
            Uuid::from(data::id::<GroupId>(1)),
            &GoalAction::Pause {}
        )
        .await
        .is_err()
    );
    tx.rollback().await.expect("rollback");
    let mut tx = f.raw.begin().await.expect("transaction");
    // An empty observation must never create an installation, even if a caller skips policy.
    record_installation(
        &mut tx,
        data::id(1),
        data::id(2),
        data::id(300),
        &LearningDecision::Activate {},
        &policy::create(data::goal(), data::id(2)),
    )
    .await
    .expect("empty observation");
    let mut view = policy::create(data::goal(), data::id(2));
    view.learning
        .push(orchestrator_protocol::collaboration::Learning {
            task_id: data::id(202),
            node_id: data::id(9),
            release: data::release(300),
            rationale: "proposal".to_owned().try_into().expect("text"),
            status: orchestrator_protocol::collaboration::LearningStatus::Proposed {},
        });
    record_installation(
        &mut tx,
        data::id(1),
        data::id(2),
        data::id(300),
        &LearningDecision::Activate {},
        &view,
    )
    .await
    .expect("unrelated proposal");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM node_capabilities")
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    assert_eq!(count, 0);
    tx.rollback().await.expect("rollback");
    f.close().await;
}
