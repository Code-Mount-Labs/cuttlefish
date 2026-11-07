use crate::tests::{
    collaboration::TestPlanner,
    common::{Fixture, Server},
    data,
    delivery::runtime,
    routing::Selection,
};
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use data::{change, goal, id, task};
use orchestrator_coordinator::web;
use orchestrator_node::sync::Remote;
use orchestrator_protocol::collaboration::GoalAction;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn malformed_contexts_and_acknowledgments_never_discard_pending_work() {
    for (path, status, body) in [
        ("/v1/goals/", StatusCode::BAD_GATEWAY, "{}".to_owned()),
        ("/v1/goals/", StatusCode::OK, "{}".into()),
        ("/v1/goals/", StatusCode::OK, "invalid json".into()),
        ("/v1/goals/", StatusCode::OK, "x".repeat(4_194_305)),
        ("/v1/goals/", StatusCode::FOUND, "{}".into()),
        ("/v1/nodes", StatusCode::BAD_GATEWAY, "{}".into()),
        ("/v1/nodes", StatusCode::OK, "{}".into()),
        (
            "/v1/capabilities/installations",
            StatusCode::BAD_GATEWAY,
            "{}".into(),
        ),
        (
            "/v1/capabilities/installations",
            StatusCode::OK,
            "{}".into(),
        ),
        ("/v1/capabilities", StatusCode::BAD_GATEWAY, "{}".into()),
        ("/v1/capabilities", StatusCode::OK, "{}".into()),
        ("/commands", StatusCode::OK, "{}".into()),
        (
            "/commands",
            StatusCode::OK,
            json!(orchestrator_domain::collaboration::create(goal(), id(2))).to_string(),
        ),
        ("/commands", StatusCode::SERVICE_UNAVAILABLE, "{}".into()),
    ] {
        context_fault(path, status, body).await;
    }
}
async fn context_fault(path: &'static str, status: StatusCode, body: String) {
    let f = Fixture::enrolled().await;
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let server = Server::start(web::router(f.store.clone()).layer(middleware::from_fn(
        move |req: Request, next: Next| {
            let body = body.clone();
            async move {
                if req.uri().path() == path
                    || req.uri().path().ends_with(path)
                    || path == "/v1/goals/" && req.uri().path().starts_with(path)
                {
                    (status, body).into_response()
                } else {
                    next.run(req).await
                }
            }
        },
    )))
    .await;
    let (_temp, mut node, _) = runtime().await;
    let planner = TestPlanner::default();
    *planner.decision.lock().expect("decision") = Some(change(&view, GoalAction::Pause {}));
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
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
            .is_err(),
        "{path}"
    );
    assert_eq!(
        node.journal()
            .pending_goal(goal().goal_id)
            .await
            .expect("pending")
            .is_some(),
        path == "/commands"
    );
    node.journal().close().await;
    server.close().await;
    f.close().await;
}

#[tokio::test]
async fn dispatch_unavailability_can_trigger_learning_but_other_faults_are_explicit() {
    for fault in [
        "unavailable",
        "denied",
        "refreshed_json",
        "refreshed_http",
        "invalid_plan",
    ] {
        dispatch_fault(fault).await;
    }
}

async fn dispatch_fault(fault: &'static str) {
    let f = Fixture::enrolled().await;
    let view = f
        .store
        .create_goal(&data::token(), &goal())
        .await
        .expect("goal");
    let view = f
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
        .goal_command(
            &data::token(),
            goal().goal_id,
            &change(&view, GoalAction::Delegate { task_id: id(202) }),
        )
        .await
        .expect("reserve");
    let count = Arc::new(AtomicUsize::new(0));
    let server = Server::start(
        web::router_with_selector(f.store.clone(), Arc::new(Selection::default())).layer(
            middleware::from_fn(move |req: Request, next: Next| {
                let count = count.clone();
                async move {
                    let path = req.uri().path().to_owned();
                    if path.ends_with("/dispatch") {
                        if fault == "unavailable" {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        if fault == "denied" {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                    }
                    if path.starts_with("/v1/goals/") && !path.ends_with("/dispatch") {
                        if fault == "invalid_plan" {
                            let mut view =
                                orchestrator_domain::collaboration::create(goal(), id(2));
                            let mut spec = task(200);
                            spec.request.action =
                                orchestrator_protocol::routing::RouteAction::Cancel {};
                            view.tasks
                                .push(orchestrator_protocol::collaboration::TaskView {
                                    spec,
                                    dispatching: true,
                                    execution: None,
                                    progress: None,
                                    verification: None,
                                });
                            return axum::Json(view).into_response();
                        }
                        if count.fetch_add(1, Ordering::SeqCst) > 0 {
                            if fault == "refreshed_json" {
                                return axum::Json(json!({})).into_response();
                            }
                            if fault == "refreshed_http" {
                                return StatusCode::BAD_GATEWAY.into_response();
                            }
                        }
                    }
                    next.run(req).await
                }
            }),
        ),
    )
    .await;
    let (_temp, mut node, _) = runtime().await;
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    assert_eq!(
        remote
            .super_step(
                &mut node,
                &data::report().profile,
                goal().goal_id,
                &TestPlanner::default(),
                10
            )
            .await
            .is_ok(),
        fault == "unavailable",
        "{fault}"
    );
    node.journal().close().await;
    server.close().await;
    f.close().await;
}

#[tokio::test]
async fn journal_failures_during_a_step_do_not_become_success() {
    for (path, sql, status) in [
        ("/v1/dispatch/poll", "DROP TABLE super_agent_outbox", None),
        ("/v1/capabilities", "DROP TABLE workspace_mounts", None),
        ("/v1/capabilities", "DROP TABLE planning_budget", None),
        ("/v1/capabilities", "DROP TABLE super_agent_outbox", None),
        (
            "/commands",
            "DROP TABLE super_agent_outbox",
            Some(StatusCode::CONFLICT),
        ),
        ("/commands", "DROP TABLE super_agent_outbox", None),
    ] {
        let f = Fixture::enrolled().await;
        let view = f
            .store
            .create_goal(&data::token(), &goal())
            .await
            .expect("goal");
        let (temp, mut node, _) = runtime().await;
        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite://{}",
            temp.path().join("node.sqlite").display()
        ))
        .await
        .expect("local db");
        let writer = pool.clone();
        let server = Server::start(web::router(f.store.clone()).layer(middleware::from_fn(
            move |req: Request, next: Next| {
                let pool = writer.clone();
                async move {
                    let matches = req.uri().path() == path || req.uri().path().ends_with(path);
                    let response = next.run(req).await;
                    if matches {
                        sqlx::raw_sql(sql).execute(&pool).await.expect("fault");
                        if let Some(status) = status {
                            return status.into_response();
                        }
                    }
                    response
                }
            },
        )))
        .await;
        let planner = TestPlanner::default();
        *planner.decision.lock().expect("decision") = Some(change(&view, GoalAction::Pause {}));
        let remote = Remote::new(&server.origin, &data::token()).expect("remote");
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
                .is_err(),
            "{path} {sql}"
        );
        pool.close().await;
        node.journal().close().await;
        server.close().await;
        f.close().await;
    }
}

#[tokio::test]
async fn socket_loss_after_sync_is_not_a_valid_goal_response() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for partial in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            for step in 0..3 {
                let (mut stream, _) = listener.accept().await.expect("connection");
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.expect("request header"));
                    assert!(request.len() < 65_536);
                }
                let header = String::from_utf8(request).expect("header");
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().expect("length"))
                    })
                    .unwrap_or(0);
                stream.read_exact(&mut vec![0; length]).await.expect("body");
                if step == 2 {
                    assert!(header.starts_with("GET /v1/goals/"));
                    if partial {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{").await.expect("headers");
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                } else {
                    let body = if step == 0 {
                        r#"{"version":1,"acknowledged":[]}"#
                    } else {
                        r#"{"acknowledged":[],"command":null}"#
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream
                        .write_all(response.as_bytes())
                        .await
                        .expect("response");
                }
                stream.shutdown().await.expect("close");
            }
        });
        let (_temp, mut node, _) = runtime().await;
        let remote = Remote::new(&origin, &data::token()).expect("remote");
        assert!(
            remote
                .super_step(
                    &mut node,
                    &data::report().profile,
                    goal().goal_id,
                    &TestPlanner::default(),
                    10
                )
                .await
                .is_err()
        );
        server.await.expect("server");
        node.journal().close().await;
    }
}
