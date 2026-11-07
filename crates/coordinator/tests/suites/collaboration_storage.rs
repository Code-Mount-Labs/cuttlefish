use crate::tests::{
    common::Fixture,
    data,
    routing::{Selection, ready},
};
use data::{change, goal, id, release, task};
use orchestrator_coordinator::{
    dispatch::{Input, Prepared},
    routing::RoutingService,
};
use orchestrator_protocol::collaboration::{
    GoalAction, GoalView, LearningDecision, LearningStatus,
};
use std::sync::Arc;

pub(super) async fn proposal(f: &Fixture) -> GoalView {
    f.store
        .publish_release(id(1), &release(300))
        .await
        .expect("catalog");
    let mut view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    for action in [
        GoalAction::Plan {
            tasks: vec![task(200)],
        },
        GoalAction::Learn {
            task_id: id(202),
            node_id: id(2),
            release_id: id(300),
            rationale: "learn".to_owned().try_into().expect("text"),
        },
    ] {
        view = f
            .store
            .goal_command(&data::token(), goal().goal_id, &change(&view, action))
            .await
            .expect("command");
    }
    view
}

#[tokio::test]
async fn learning_and_catalog_transactions_roll_back_on_storage_faults() {
    for fault in [
        "ALTER TABLE groups RENAME COLUMN group_id TO hidden",
        "ALTER TABLE goals RENAME COLUMN snapshot TO hidden",
        "ALTER TABLE node_capabilities RENAME COLUMN active TO hidden",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail_write BEFORE UPDATE ON goals FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "ALTER TABLE goal_commands ADD CONSTRAINT reject_command CHECK(FALSE) NOT VALID",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER INSERT ON goal_commands DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
    ] {
        let f = Fixture::enrolled().await;
        let view = proposal(&f).await;
        f.sql(fault).await;
        let decision = LearningDecision::Reject {
            reason: "rejected".to_owned().try_into().expect("text"),
        };
        assert!(
            f.store
                .decide_learning(
                    id(1),
                    goal().goal_id,
                    view.revision,
                    id(2),
                    id(300),
                    &decision
                )
                .await
                .is_err(),
            "{fault}"
        );
        f.close().await;
    }
    for fault in [
        "ALTER TABLE groups RENAME COLUMN group_id TO hidden",
        "ALTER TABLE capability_releases RENAME COLUMN manifest TO hidden",
        "ALTER TABLE capability_releases ADD CONSTRAINT reject_release CHECK(FALSE)",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER INSERT ON capability_releases DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
    ] {
        let f = Fixture::enrolled().await;
        f.sql(fault).await;
        assert!(
            f.store.publish_release(id(1), &release(300)).await.is_err(),
            "{fault}"
        );
        f.close().await;
    }
}

#[tokio::test]
async fn active_capabilities_are_durable_and_revocable() {
    let f = Fixture::enrolled().await;
    let mut view = proposal(&f).await;
    view = f
        .store
        .decide_learning(
            id(1),
            goal().goal_id,
            view.revision,
            id(2),
            id(300),
            &LearningDecision::Approve {},
        )
        .await
        .expect("approve");
    view = f
        .store
        .goal_command(
            &data::token(),
            goal().goal_id,
            &change(
                &view,
                GoalAction::Evaluate {
                    release_id: id(300),
                    passed: true,
                    evidence: "evaluation passed".to_owned().try_into().expect("text"),
                },
            ),
        )
        .await
        .expect("evaluate");
    f.sql("ALTER TABLE node_capabilities ADD CONSTRAINT reject_install CHECK(FALSE)")
        .await;
    assert!(
        f.store
            .decide_learning(
                id(1),
                goal().goal_id,
                view.revision,
                id(2),
                id(300),
                &LearningDecision::Activate {}
            )
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .goal(&data::token(), goal().goal_id)
            .await
            .expect("rollback"),
        view
    );
    f.sql("ALTER TABLE node_capabilities DROP CONSTRAINT reject_install")
        .await;
    view = f
        .store
        .decide_learning(
            id(1),
            goal().goal_id,
            view.revision,
            id(2),
            id(300),
            &LearningDecision::Activate {},
        )
        .await
        .expect("activate");
    let installs = f
        .store
        .installations(&data::token())
        .await
        .expect("registry");
    assert_eq!(installs.len(), 1);
    assert_eq!(installs[0].release, release(300));
    view = f
        .store
        .decide_learning(
            id(1),
            goal().goal_id,
            view.revision,
            id(2),
            id(300),
            &LearningDecision::Reject {
                reason: "revoke".to_owned().try_into().expect("text"),
            },
        )
        .await
        .expect("revoke");
    assert!(matches!(
        view.learning[0].status,
        LearningStatus::Rejected { .. }
    ));
    assert!(
        f.store
            .installations(&data::token())
            .await
            .expect("registry")
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn reads_and_creates_fail_closed_on_corrupt_or_unavailable_storage() {
    for fault in [
        "ALTER TABLE goals ADD CONSTRAINT reject_goal CHECK(FALSE)",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER INSERT ON goals DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
        "UPDATE nodes SET node_id='00000000-0000-0000-0000-000000000000'",
    ] {
        let f = Fixture::enrolled().await;
        f.sql(fault).await;
        assert!(f.store.create_goal(&data::token(), &goal()).await.is_err());
        f.close().await;
    }
    for fault in [
        "ALTER TABLE goals RENAME COLUMN goal_id TO hidden",
        "UPDATE goals SET goal_id='00000000-0000-0000-0000-000000000000'",
        "UPDATE goals SET snapshot='{}'",
        "ALTER TABLE dispatches RENAME COLUMN outcome TO hidden",
    ] {
        let f = Fixture::enrolled().await;
        f.store
            .create_goal(&data::token(), &goal())
            .await
            .expect("goal");
        f.sql(fault).await;
        assert!(f.store.goals(&data::token()).await.is_err(), "{fault}");
        f.close().await;
    }
    let f = Fixture::enrolled().await;
    f.sql("ALTER TABLE node_capabilities RENAME COLUMN active TO hidden")
        .await;
    assert!(f.store.installations(&data::token()).await.is_err());
    assert!(
        f.store
            .delegation(&data::token(), goal().goal_id, id(202))
            .await
            .is_err()
    );
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    assert!(
        f.store
            .delegation(&data::token(), goal().goal_id, id(202))
            .await
            .is_err()
    );
    let other = "b".repeat(64).try_into().expect("token");
    f.store.enroll(id(1), id(9), &other).await.expect("enroll");
    f.sql("UPDATE nodes SET node_id='00000000-0000-0000-0000-000000000000' WHERE node_id='00000000-0000-0000-0000-000000000009'").await;
    assert!(
        f.store
            .goal_command(&other, goal().goal_id, &change(&view, GoalAction::Pause {}))
            .await
            .is_err()
    );
    f.sql("ALTER TABLE nodes RENAME COLUMN revoked TO hidden")
        .await;
    assert!(
        f.store
            .goal_command(
                &data::token(),
                goal().goal_id,
                &change(&view, GoalAction::Pause {})
            )
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn task_links_cannot_adopt_existing_work_and_ownership_is_rechecked() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let router = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    router
        .submit(&token, task(200).request)
        .await
        .expect("existing execution");
    let mut view = f.store.create_goal(&token, &goal()).await.expect("goal");
    assert!(
        f.store
            .goal_command(
                &token,
                goal().goal_id,
                &change(
                    &view,
                    GoalAction::Plan {
                        tasks: vec![task(200)]
                    }
                )
            )
            .await
            .is_err()
    );
    view = f
        .store
        .goal_command(
            &token,
            goal().goal_id,
            &change(
                &view,
                GoalAction::Plan {
                    tasks: vec![task(210)],
                },
            ),
        )
        .await
        .expect("plan");
    view = f
        .store
        .goal_command(
            &token,
            goal().goal_id,
            &change(&view, GoalAction::Delegate { task_id: id(212) }),
        )
        .await
        .expect("reserve");
    let prepared = f
        .store
        .prepare(&token, Input::Submit(task(210).request.clone()))
        .await
        .expect("prepare");
    let plan = match prepared {
        Prepared::Choose(plan) => Some(plan),
        Prepared::Ready(_) => None,
    }
    .expect("expected selection");
    f.store
        .goal_command(&token, goal().goal_id, &change(&view, GoalAction::Pause {}))
        .await
        .expect("pause while model runs");
    assert!(f.store.commit_route(&token, *plan, id(2)).await.is_err());
    assert!(
        router
            .submit(&token, task(210).request.clone())
            .await
            .is_err()
    );
    f.sql("UPDATE goals SET snapshot=jsonb_set(snapshot,'{status}','\"active\"')")
        .await;
    router
        .delegate(&token, goal().goal_id, id(212))
        .await
        .expect("delegate");
    router
        .failover(&token, task(210).request.thread_id)
        .await
        .expect("healthy owner retains original assignment");
    let mut follow = task(210).request;
    follow.command_id = id(999);
    follow.action = orchestrator_protocol::routing::RouteAction::Cancel {};
    router
        .submit(&token, follow)
        .await
        .expect_err("original start is still pending");
    f.sql("UPDATE goals SET snapshot=jsonb_set(snapshot,'{tasks,0,dispatching}','false')")
        .await;
    assert!(f.store.goal(&token, goal().goal_id).await.is_err());
    f.close().await;
}
