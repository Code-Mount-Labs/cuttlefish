use crate::tests::{
    collaboration::TestPlanner,
    common::{Fixture, Server},
    data,
    delivery::runtime,
    routing::Selection,
};
use data::{change, goal, id, task};
use orchestrator_coordinator::web;
use orchestrator_node::sync::Remote;
use orchestrator_protocol::collaboration::{GoalAction, GoalCommand, GoalStatus};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

#[tokio::test]
async fn owner_loop_plans_dispatches_and_honors_pause_and_durable_budget() {
    let f = Fixture::enrolled().await;
    let mut view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let server = Server::start(web::router_with_selector(
        f.store.clone(),
        Arc::new(Selection::default()),
    ))
    .await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let (_temp, mut node, _) = runtime().await;
    let planner = TestPlanner::default();
    *planner.decision.lock().expect("decision") = Some(change(
        &view,
        GoalAction::Plan {
            tasks: vec![task(200)],
        },
    ));
    remote
        .super_step(
            &mut node,
            &data::report().profile,
            goal().goal_id,
            &planner,
            5,
        )
        .await
        .expect("plan");
    view = f
        .store
        .goal(&data::token(), goal().goal_id)
        .await
        .expect("refresh");
    *planner.decision.lock().expect("decision") =
        Some(change(&view, GoalAction::Delegate { task_id: id(202) }));
    remote
        .super_step(
            &mut node,
            &data::report().profile,
            goal().goal_id,
            &planner,
            5,
        )
        .await
        .expect("reserve");
    remote
        .super_step(
            &mut node,
            &data::report().profile,
            goal().goal_id,
            &planner,
            5,
        )
        .await
        .expect("dispatch");
    let view = f
        .store
        .goal(&data::token(), goal().goal_id)
        .await
        .expect("view");
    assert!(view.tasks[0].execution.is_some());
    check_paused(&f, &remote, &mut node, &planner, &view).await;
    node.journal().close().await;
    server.close().await;
    f.close().await;
}

async fn check_paused(
    f: &Fixture,
    remote: &Remote,
    node: &mut orchestrator_node::runtime::NodeRuntime<crate::tests::delivery::DeliveryLauncher>,
    planner: &TestPlanner,
    view: &orchestrator_protocol::collaboration::GoalView,
) {
    let paused = f
        .store
        .goal_command(
            &data::token(),
            goal().goal_id,
            &change(view, GoalAction::Pause {}),
        )
        .await
        .expect("pause");
    assert_eq!(paused.status, GoalStatus::Paused);
    remote
        .super_step(node, &data::report().profile, goal().goal_id, planner, 5)
        .await
        .expect("paused");
    assert_eq!(planner.calls.load(Ordering::SeqCst), 3);
    assert!(
        node.journal()
            .reserve_turn(goal().goal_id, 3)
            .await
            .is_err()
    );
    assert!(
        node.journal()
            .reserve_turn(goal().goal_id, 0)
            .await
            .is_err()
    );
    // A pending stale decision is retired only after an explicit server conflict.
    node.journal()
        .stage_goal(goal().goal_id, &change(view, GoalAction::Resume {}))
        .await
        .expect("stale outbox");
    assert!(
        remote
            .super_step(node, &data::report().profile, goal().goal_id, planner, 5)
            .await
            .is_err()
    );
    assert!(
        node.journal()
            .pending_goal(goal().goal_id)
            .await
            .expect("retired")
            .is_none()
    );
}

#[tokio::test]
async fn owner_loop_rejects_wrong_owners_invalid_decisions_and_model_failures() {
    let f = Fixture::enrolled().await;
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let server = Server::start(web::router(f.store.clone())).await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let (_temp, mut node, _) = runtime().await;
    let bad = TestPlanner {
        fail: true,
        ..TestPlanner::default()
    };
    assert!(
        remote
            .super_step(&mut node, &data::report().profile, goal().goal_id, &bad, 10)
            .await
            .is_err()
    );
    let planner = TestPlanner::default();
    *planner.decision.lock().expect("decision") = Some(change(
        &view,
        GoalAction::Finish {
            result: "no work".to_owned().try_into().expect("text"),
        },
    ));
    assert!(
        remote
            .super_step(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &planner,
                10
            )
            .await
            .is_err()
    );
    assert!(
        node.journal()
            .pending_goal(goal().goal_id)
            .await
            .expect("outbox")
            .is_none()
    );
    f.sql("UPDATE goals SET snapshot=jsonb_set(snapshot,'{owner}','\"00000000-0000-0000-0000-000000000009\"')").await;
    assert!(
        remote
            .super_step(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &planner,
                10
            )
            .await
            .is_err()
    );
    node.journal().close().await;
    assert!(
        remote
            .super_step(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &planner,
                10
            )
            .await
            .is_err()
    );
    server.close().await;
    f.close().await;
}

#[tokio::test]
async fn supervision_waits_and_returns_signal_and_step_errors() {
    let f = Fixture::enrolled().await;
    f.store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let server = Server::start(web::router(f.store.clone())).await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let (_temp, mut node, _) = runtime().await;
    let planner = TestPlanner::default();
    let shutdown: Pin<Box<dyn Future<Output = std::io::Result<()>>>> = Box::pin(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    });
    remote
        .supervise(
            &mut node,
            &data::report().profile,
            goal().goal_id,
            &planner,
            10,
            shutdown,
        )
        .await
        .expect("shutdown");
    assert_eq!(planner.calls.load(Ordering::SeqCst), 1);
    let shutdown: Pin<Box<dyn Future<Output = std::io::Result<()>>>> =
        Box::pin(async { Err(std::io::Error::other("signal")) });
    assert!(
        remote
            .supervise(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &planner,
                10,
                shutdown
            )
            .await
            .is_err()
    );
    node.journal().close().await;
    let shutdown: Pin<Box<dyn Future<Output = std::io::Result<()>>>> =
        Box::pin(std::future::pending());
    assert!(
        remote
            .supervise(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &planner,
                10,
                shutdown
            )
            .await
            .is_err()
    );
    server.close().await;
    f.close().await;
}

#[tokio::test]
async fn local_decisions_are_immutable_and_storage_failures_are_explicit() {
    let (_temp, node, _) = runtime().await;
    let view = orchestrator_domain::collaboration::create(goal(), id(2));
    let command = change(&view, GoalAction::Pause {});
    node.journal()
        .stage_goal(goal().goal_id, &command)
        .await
        .expect("stage");
    node.journal()
        .stage_goal(goal().goal_id, &command)
        .await
        .expect("repeat");
    let other = GoalCommand {
        action: GoalAction::Resume {},
        ..command
    };
    assert!(
        node.journal()
            .stage_goal(goal().goal_id, &other)
            .await
            .is_err()
    );
    node.journal().close().await;
    assert!(
        node.journal()
            .stage_goal(goal().goal_id, &other)
            .await
            .is_err()
    );
    assert!(node.journal().pending_goal(goal().goal_id).await.is_err());
    assert!(node.journal().clear_goal(goal().goal_id).await.is_err());
    assert!(
        node.journal()
            .reserve_turn(goal().goal_id, 10)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn restarting_the_supervisor_does_not_reset_the_planning_budget() {
    let (temp, node, launcher) = runtime().await;
    let identity = node.journal().identity();
    node.journal()
        .reserve_turn(goal().goal_id, 1)
        .await
        .expect("first call");
    node.journal().close().await;
    drop(node);
    let replacement =
        orchestrator_node::runtime::NodeRuntime::open(temp.path(), identity, launcher)
            .await
            .expect("restart");
    assert!(
        replacement
            .journal()
            .reserve_turn(goal().goal_id, 1)
            .await
            .is_err()
    );
    replacement
        .journal()
        .reserve_turn(goal().goal_id, 2)
        .await
        .expect("explicitly increased budget");
    replacement.journal().close().await;
}
