use crate::tests::{
    common::{Fixture, Server},
    data,
    delivery::runtime,
    routing::{Selection, finish, poll, ready},
};
use data::{change, goal, id, release, task};
use orchestrator_agents::planner::{Planner, PlannerError, PlanningContext};
use orchestrator_coordinator::{routing::RoutingService, store::Store, web};
use orchestrator_node::{model::NodeIdentity, runtime::NodeRuntime, sync::Remote};
use orchestrator_protocol::{collaboration::*, sync::NodeToken};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn text(value: &str) -> orchestrator_protocol::values::MessageText {
    value.to_owned().try_into().expect("text")
}
async fn apply(f: &Fixture, token: &NodeToken, view: &GoalView, action: GoalAction) -> GoalView {
    f.store
        .goal_command(token, view.spec.goal_id, &change(view, action))
        .await
        .expect("goal action")
}
#[tokio::test]
async fn goal_delegates_to_peers_tracks_progress_and_survives_router_restart() {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 4).await;
    let b = ready(&f, 9, 4).await;
    let selector = Arc::new(Selection::default());
    let router = RoutingService::new(f.store.clone(), selector.clone());
    let mut view = f.store.create_goal(&a, &goal()).await.expect("goal");
    assert_eq!(f.store.create_goal(&a, &goal()).await.expect("retry"), view);
    let mut second = task(210);
    second.dependencies.insert(id(202));
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Plan {
            tasks: vec![task(200), second],
        },
    )
    .await;
    assert!(router.submit(&a, task(200).request).await.is_err());
    apply(&f, &a, &view, GoalAction::Delegate { task_id: id(202) }).await;
    assert!(router.delegate(&b, goal().goal_id, id(202)).await.is_err());
    let first = router
        .delegate(&a, goal().goal_id, id(202))
        .await
        .expect("first");
    assert_eq!(
        selector.contexts.lock().expect("context")[0]
            .active_work
            .len(),
        2
    );
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    finish(&f, &a, &first, 1).await;
    view = f.store.goal(&a, goal().goal_id).await.expect("hydrate");
    assert!(
        view.tasks[0]
            .execution
            .as_ref()
            .expect("execution")
            .outcome
            .is_some()
    );
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Progress {
            task_id: id(202),
            progress: Progress {
                completed: 1,
                total: 1,
                note: text("tested"),
            },
        },
    )
    .await;
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Verify {
            task_id: id(202),
            evidence: text("checked acceptance"),
        },
    )
    .await;
    apply(&f, &a, &view, GoalAction::Delegate { task_id: id(212) }).await;
    finish_peer(f, a, b).await;
}

async fn finish_peer(f: Fixture, a: NodeToken, b: NodeToken) {
    let peer_router = RoutingService::new(
        f.store.clone(),
        Arc::new(Selection {
            destination: Some(id(9)),
            ..Selection::default()
        }),
    );
    let second = peer_router
        .delegate(&a, goal().goal_id, id(212))
        .await
        .expect("peer");
    f.store
        .poll_dispatch(&b, &poll())
        .await
        .expect("claim peer");
    finish(&f, &b, &second, 1).await;
    let replica = Store::open(&f.url).await.expect("restart router");
    let mut view = replica.goal(&a, goal().goal_id).await.expect("recovered");
    assert_eq!(
        view.tasks[1]
            .execution
            .as_ref()
            .expect("peer")
            .command
            .node_id,
        id(9)
    );
    view = apply(
        &f,
        &b,
        &view,
        GoalAction::Discuss {
            recipient: Some(id(2)),
            text: text("peer has completed the artifact"),
        },
    )
    .await;
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Verify {
            task_id: id(212),
            evidence: text("peer output checked"),
        },
    )
    .await;
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Finish {
            result: text("verified all artifacts"),
        },
    )
    .await;
    assert_eq!(replica.goals(&a).await.expect("goals"), vec![view]);
    replica.close().await;
    f.close().await;
}

#[tokio::test]
async fn learning_approval_and_replication_are_scoped_to_each_target() {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 4).await;
    let b = ready(&f, 9, 4).await;
    f.store
        .publish_release(id(1), &release(300))
        .await
        .expect("catalog");
    f.store
        .publish_release(id(1), &release(300))
        .await
        .expect("catalog retry");
    assert_eq!(
        f.store.catalog(&a).await.expect("discovery"),
        vec![release(300)]
    );
    let mut view = f.store.create_goal(&a, &goal()).await.expect("goal");
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Plan {
            tasks: vec![task(200)],
        },
    )
    .await;
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Learn {
            task_id: id(202),
            node_id: id(2),
            release_id: id(300),
            rationale: text("missing capability"),
        },
    )
    .await;
    assert!(
        f.store
            .decide_learning(
                id(1),
                goal().goal_id,
                view.revision - 1,
                id(2),
                id(300),
                &LearningDecision::Approve {}
            )
            .await
            .is_err()
    );
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
        .expect("human approval");
    finish_learning(f, a, b, view).await;
}

async fn finish_learning(f: Fixture, a: NodeToken, b: NodeToken, mut view: GoalView) {
    let replica = Store::open(&f.url).await.expect("restart");
    assert_eq!(
        replica
            .goal(&a, goal().goal_id)
            .await
            .expect("recover approval"),
        view
    );
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Evaluate {
            release_id: id(300),
            passed: true,
            evidence: text("local installation verified by conformance suite"),
        },
    )
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
        .expect("activation");
    replicate(f, a, b, view, replica).await;
}

async fn replicate(f: Fixture, a: NodeToken, b: NodeToken, mut view: GoalView, replica: Store) {
    view = apply(
        &f,
        &a,
        &view,
        GoalAction::Replicate {
            release_id: id(300),
            nodes: [id(9)].into(),
            rationale: text("requested peer replication"),
        },
    )
    .await;
    assert!(matches!(
        view.learning[1].status,
        LearningStatus::Proposed {}
    ));
    assert!(
        f.store
            .goal_command(
                &b,
                goal().goal_id,
                &change(
                    &view,
                    GoalAction::Evaluate {
                        release_id: id(300),
                        passed: true,
                        evidence: text("not approved yet")
                    }
                )
            )
            .await
            .is_err()
    );
    view = f
        .store
        .decide_learning(
            id(1),
            goal().goal_id,
            view.revision,
            id(9),
            id(300),
            &LearningDecision::Approve {},
        )
        .await
        .expect("separate approval");
    view = apply(
        &f,
        &b,
        &view,
        GoalAction::Evaluate {
            release_id: id(300),
            passed: false,
            evidence: text("missing platform dependency"),
        },
    )
    .await;
    assert!(
        f.store
            .decide_learning(
                id(1),
                goal().goal_id,
                view.revision,
                id(9),
                id(300),
                &LearningDecision::Activate {}
            )
            .await
            .is_err()
    );
    let mut improved = release(301);
    improved.supersedes = Some(id(300));
    f.store
        .publish_release(id(1), &improved)
        .await
        .expect("new immutable harness version");
    assert!(matches!(
        replica
            .goal(&a, goal().goal_id)
            .await
            .expect("original unchanged")
            .learning[0]
            .status,
        LearningStatus::Active {}
    ));
    replica.close().await;
    f.close().await;
}

#[derive(Debug, Default)]
pub(super) struct TestPlanner {
    pub calls: AtomicUsize,
    pub decision: Mutex<Option<GoalCommand>>,
    pub fail: bool,
}
impl Planner for TestPlanner {
    async fn decide(&self, _: &PlanningContext) -> Result<Option<GoalCommand>, PlannerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(PlannerError);
        }
        Ok(self.decision.lock().expect("decision").take())
    }
}

#[tokio::test]
async fn pending_decision_survives_supervisor_restart_and_lost_response() {
    use axum::{extract::Request, http::StatusCode, middleware, response::IntoResponse};
    let f = Fixture::enrolled().await;
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let command = change(
        &view,
        GoalAction::Plan {
            tasks: vec![task(200), task(210)],
        },
    );
    let failing = Server::start(web::router(f.store.clone()).layer(middleware::from_fn(
        |request: Request, next: middleware::Next| async move {
            let fail = request.uri().path().ends_with("/commands");
            let response = next.run(request).await;
            if fail {
                StatusCode::SERVICE_UNAVAILABLE.into_response()
            } else {
                response
            }
        },
    )))
    .await;
    let (temp, mut node, launcher) = runtime().await;
    // Stage explicitly at the crash boundary before sending to the first replica.
    node.journal()
        .stage_goal(goal().goal_id, &command)
        .await
        .expect("durable decision");
    let planner = TestPlanner::default();
    let remote = Remote::new(&failing.origin, &data::token()).expect("remote");
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
    drop(node);
    failing.close().await;
    let server = Server::start(web::router(f.store.clone())).await;
    let remote = Remote::new(&server.origin, &data::token()).expect("replacement router");
    let mut replacement = NodeRuntime::open(
        temp.path(),
        NodeIdentity {
            group_id: id(1),
            node_id: id(2),
        },
        launcher,
    )
    .await
    .expect("restart");
    remote
        .super_step(
            &mut replacement,
            &data::report().profile,
            goal().goal_id,
            &planner,
            10,
        )
        .await
        .expect("retry original operation");
    assert!(
        replacement
            .journal()
            .pending_goal(goal().goal_id)
            .await
            .expect("outbox")
            .is_none()
    );
    let view = f
        .store
        .goal(&data::token(), goal().goal_id)
        .await
        .expect("saved once");
    assert_eq!(view.tasks.len(), 2);
    assert_eq!(view.revision, 1);
    assert_eq!(planner.calls.load(Ordering::SeqCst), 0);
    assert!(
        f.store.nodes(&data::token()).await.expect("native")[0]
            .profile
            .as_ref()
            .expect("profile")
            .capabilities
            .contains(&"agent.learn".to_owned().try_into().expect("capability"))
    );
    replacement.journal().close().await;
    server.close().await;
    f.close().await;
}
