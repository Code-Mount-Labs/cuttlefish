use crate::tests::{
    common::{Fixture, Server},
    data,
};
use data::{change, goal, id, release, task};
use orchestrator_coordinator::{store::Store, web};
use orchestrator_protocol::collaboration::{GoalAction, GoalView, LearningDecision};

#[tokio::test]
async fn http_goals_catalog_and_learning_are_authenticated_and_group_scoped() {
    let f = Fixture::enrolled().await;
    let server = Server::start(web::router(f.store.clone())).await;
    check_http_goals(&f, &server).await;
    server.close().await;
    f.close().await;
}

async fn check_http_goals(f: &Fixture, server: &Server) {
    let client = reqwest::Client::new();
    check_anonymous_reads(server, &client).await;
    assert_eq!(
        client
            .post(format!("{}/v1/goals", server.origin))
            .json(&goal())
            .send()
            .await
            .expect("http")
            .status(),
        401
    );
    let view: GoalView = client
        .post(format!("{}/v1/goals", server.origin))
        .bearer_auth(data::token().expose())
        .json(&goal())
        .send()
        .await
        .expect("create")
        .json()
        .await
        .expect("view");
    let command = change(
        &view,
        GoalAction::Plan {
            tasks: vec![task(200)],
        },
    );
    let path = format!("{}/v1/goals/{}/commands", server.origin, goal().goal_id);
    assert_eq!(
        client
            .post(&path)
            .json(&command)
            .send()
            .await
            .expect("http")
            .status(),
        401
    );
    for _ in 0..2 {
        assert_eq!(
            client
                .post(&path)
                .bearer_auth(data::token().expose())
                .json(&command)
                .send()
                .await
                .expect("http")
                .status(),
            200
        );
    }
    let conflict = orchestrator_protocol::collaboration::GoalCommand {
        action: GoalAction::Pause {},
        ..command
    };
    assert_eq!(
        client
            .post(&path)
            .bearer_auth(data::token().expose())
            .json(&conflict)
            .send()
            .await
            .expect("http")
            .status(),
        409
    );
    for path in [
        "/v1/goals".into(),
        format!("/v1/goals/{}", goal().goal_id),
        "/v1/capabilities".into(),
        "/v1/capabilities/installations".into(),
    ] {
        assert_eq!(
            client
                .get(format!("{}{path}", server.origin))
                .bearer_auth(data::token().expose())
                .send()
                .await
                .expect("http")
                .status(),
            200
        );
    }
    check_dispatch_and_unavailable(f, server, &client).await;
}

async fn check_dispatch_and_unavailable(f: &Fixture, server: &Server, client: &reqwest::Client) {
    let dispatch = format!(
        "{}/v1/goals/{}/tasks/{}/dispatch",
        server.origin,
        goal().goal_id,
        id::<orchestrator_protocol::ids::TaskId>(202)
    );
    assert_eq!(
        client.post(&dispatch).send().await.expect("http").status(),
        401
    );
    assert_eq!(
        client
            .post(&dispatch)
            .bearer_auth(data::token().expose())
            .send()
            .await
            .expect("http")
            .status(),
        409
    );
    assert_eq!(
        client
            .post(format!(
                "{}/v1/goals/{}/approve",
                server.origin,
                goal().goal_id
            ))
            .bearer_auth(data::token().expose())
            .send()
            .await
            .expect("http")
            .status(),
        404
    );
    f.store.close().await;
    for path in [
        "/v1/goals".into(),
        format!("/v1/goals/{}", goal().goal_id),
        "/v1/capabilities".into(),
        "/v1/capabilities/installations".into(),
    ] {
        assert_eq!(
            client
                .get(format!("{}{path}", server.origin))
                .bearer_auth(data::token().expose())
                .send()
                .await
                .expect("http")
                .status(),
            503
        );
    }
    assert_eq!(
        client
            .post(format!("{}/v1/goals", server.origin))
            .bearer_auth(data::token().expose())
            .json(&goal())
            .send()
            .await
            .expect("http")
            .status(),
        503
    );
}

#[tokio::test]
async fn invalid_catalog_requests_and_goal_ids_are_rejected() {
    let f = Fixture::enrolled().await;
    let mut view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let mut changed = goal();
    changed.objective = "changed".to_owned().try_into().expect("text");
    assert!(f.store.create_goal(&data::token(), &changed).await.is_err());
    assert!(f.store.goal(&data::token(), id(999)).await.is_err());
    view = f
        .store
        .goal_command(
            &data::token(),
            goal().goal_id,
            &change(
                &view,
                GoalAction::Plan {
                    tasks: vec![task(200)],
                },
            ),
        )
        .await
        .expect("plan");
    f.store
        .publish_release(id(1), &release(300))
        .await
        .expect("catalog");
    let mut changed = release(300);
    changed.recipe = "changed".to_owned().try_into().expect("text");
    assert!(f.store.publish_release(id(1), &changed).await.is_err());
    changed.release_id = id(301);
    changed.supersedes = Some(id(999));
    assert!(f.store.publish_release(id(1), &changed).await.is_err());
    let learn = GoalAction::Learn {
        task_id: id(202),
        node_id: id(999),
        release_id: id(300),
        rationale: "unknown target".to_owned().try_into().expect("text"),
    };
    assert!(
        f.store
            .goal_command(&data::token(), goal().goal_id, &change(&view, learn))
            .await
            .is_err()
    );
    let outsider = "f".repeat(64).try_into().expect("token");
    f.store
        .enroll(id(50), id(51), &outsider)
        .await
        .expect("outsider");
    assert!(f.store.goal(&outsider, goal().goal_id).await.is_err());
    assert!(
        f.store
            .catalog(&outsider)
            .await
            .expect("isolated catalog")
            .is_empty()
    );
    f.close().await;
}

#[tokio::test]
async fn goal_command_faults_roll_back_snapshot_tasks_and_receipts() {
    for fault in [
        "ALTER TABLE goal_commands RENAME COLUMN request TO hidden",
        "ALTER TABLE goals RENAME COLUMN snapshot TO hidden",
        "ALTER TABLE capability_releases RENAME COLUMN manifest TO hidden",
        "ALTER TABLE goal_tasks ADD CONSTRAINT reject_task CHECK(FALSE)",
        "CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail_write BEFORE UPDATE ON goals FOR EACH ROW EXECUTE FUNCTION reject_write()",
        "ALTER TABLE goal_commands ADD CONSTRAINT reject_command CHECK(FALSE)",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER INSERT ON goal_commands DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
    ] {
        let f = Fixture::enrolled().await;
        let view = f
            .store
            .create_goal(&data::token(), &goal())
            .await
            .expect("goal");
        f.sql(fault).await;
        assert!(
            f.store
                .goal_command(
                    &data::token(),
                    goal().goal_id,
                    &change(
                        &view,
                        GoalAction::Plan {
                            tasks: vec![task(200)]
                        }
                    )
                )
                .await
                .is_err(),
            "{fault}"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM goal_tasks")
            .fetch_one(&f.raw)
            .await
            .expect("count");
        assert_eq!(count, 0);
        f.close().await;
    }
}

#[tokio::test]
async fn unavailable_collaboration_storage_fails_explicitly() {
    let f = Fixture::enrolled().await;
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    f.store.close().await;
    assert!(f.store.create_goal(&data::token(), &goal()).await.is_err());
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
    assert!(
        f.store
            .delegation(&data::token(), goal().goal_id, id(202))
            .await
            .is_err()
    );
    assert!(f.store.publish_release(id(1), &release(300)).await.is_err());
    assert!(
        f.store
            .decide_learning(
                id(1),
                goal().goal_id,
                0,
                id(2),
                id(300),
                &LearningDecision::Approve {}
            )
            .await
            .is_err()
    );
    let replica = Store::open(&f.url).await.expect("replica");
    replica.close().await;
    f.close().await;
}

async fn check_anonymous_reads(server: &Server, client: &reqwest::Client) {
    for path in [
        "/v1/goals".into(),
        format!("/v1/goals/{}", goal().goal_id),
        "/v1/capabilities".into(),
        "/v1/capabilities/installations".into(),
    ] {
        assert_eq!(
            client
                .get(format!("{}{path}", server.origin))
                .send()
                .await
                .expect("http")
                .status(),
            401
        );
    }
}
