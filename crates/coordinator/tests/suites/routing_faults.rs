use crate::tests::{
    common::{Fixture, Server},
    data,
    routing::{Selection, id, poll, ready, request},
};
use orchestrator_coordinator::{Error, routing::RoutingService, selector::Unconfigured, web};
use orchestrator_protocol::{
    routing::{DispatchPoll, RouteAction},
    sync::CommandOutcome,
};
use std::sync::Arc;

#[tokio::test]
async fn invalid_lifecycles_identities_polls_and_http_failures_are_explicit() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    assert!(service.failover(&token, id(999)).await.is_err());
    let req = request(800);
    let saved = service.submit(&token, req.clone()).await.expect("start");
    let mut same_task = request(900);
    if let RouteAction::Start { task_id, .. } = &mut same_task.action {
        *task_id = saved.command.target.task_id;
    }
    assert!(matches!(
        service.submit(&token, same_task).await,
        Err(Error::RoutingConflict(_))
    ));
    let mut too_many = request(1000);
    if let RouteAction::Start { capabilities, .. } = &mut too_many.action {
        *capabilities = (0..65)
            .map(|i| format!("cap{i}").try_into().expect("cap"))
            .collect();
    }
    assert!(service.submit(&token, too_many).await.is_err());
    for invalid in [
        DispatchPoll {
            workspaces: vec![id(5); 1025],
            ..poll()
        },
        DispatchPoll {
            accepted: vec![id(800); 33],
            ..poll()
        },
        DispatchPoll {
            accepted: vec![id(800); 2],
            ..poll()
        },
        DispatchPoll {
            accepted: vec![id(999)],
            ..poll()
        },
        DispatchPoll {
            accepted: vec![id(800)],
            ..poll()
        },
    ] {
        assert!(f.store.poll_dispatch(&token, &invalid).await.is_err());
    }
    let mut result = data::report();
    result.events[0].command = saved.command.clone();
    result.events[0].outcome = CommandOutcome::Rejected {
        reason: "rejected".into(),
    };
    assert!(f.store.sync(&token, &result).await.is_err()); // Never delivered.
    f.store.poll_dispatch(&token, &poll()).await.expect("claim");
    f.store.sync(&token, &result).await.expect("result");
    f.store.sync(&token, &result).await.expect("exact replay");
    result.events[0].outcome = CommandOutcome::Rejected {
        reason: "changed".into(),
    };
    assert!(f.store.sync(&token, &result).await.is_err());
    http_errors(&f, &token, &req).await;
    f.close().await;
}

async fn http_errors(
    f: &Fixture,
    token: &orchestrator_protocol::sync::NodeToken,
    req: &orchestrator_protocol::routing::RouteRequest,
) {
    ready(f, 9, 4).await; // Multiple destinations still require a configured selector.
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    for path in [
        format!("/v1/threads/{}", req.thread_id),
        format!("/v1/threads/{}/failover", req.thread_id),
        "/v1/dispatch/poll".into(),
    ] {
        let builder = if path.ends_with(&req.thread_id.to_string()) {
            client.get(format!("{}{path}", server.origin))
        } else {
            client
                .post(format!("{}{path}", server.origin))
                .json(&poll())
        };
        assert_eq!(builder.send().await.expect("http").status(), 401);
    }
    for (path, body, expected) in [
        (
            "/v1/tasks".to_owned(),
            serde_json::to_value(request(1100)).expect("json"),
            502,
        ),
        (
            format!("/v1/threads/{}/failover", req.thread_id),
            serde_json::json!({}),
            409,
        ),
        (
            "/v1/dispatch/poll".into(),
            serde_json::to_value(DispatchPoll {
                accepted: vec![id(999)],
                ..poll()
            })
            .expect("json"),
            409,
        ),
    ] {
        assert_eq!(
            client
                .post(format!("{}{path}", server.origin))
                .bearer_auth(token.expose())
                .json(&body)
                .send()
                .await
                .expect("http")
                .status(),
            expected
        );
    }
    assert_eq!(
        client
            .get(format!(
                "{}/v1/threads/{}",
                server.origin,
                id::<orchestrator_protocol::ids::ThreadId>(555)
            ))
            .bearer_auth(token.expose())
            .send()
            .await
            .expect("http")
            .status(),
        409
    );
    f.sql("UPDATE nodes SET dispatch_seen=NULL").await;
    assert_eq!(
        client
            .post(format!("{}/v1/tasks", server.origin))
            .bearer_auth(token.expose())
            .json(&request(1200))
            .send()
            .await
            .expect("http")
            .status(),
        503
    );
    server.close().await;
}

#[tokio::test]
async fn routing_read_and_write_failures_leave_no_partial_assignments() {
    for fault in [
        "ALTER TABLE dispatches RENAME COLUMN request TO broken",
        "ALTER TABLE thread_bindings RENAME COLUMN revision TO broken",
        "UPDATE nodes SET profile=jsonb_set(profile,'{capacity}','\"invalid\"')",
        "ALTER TABLE node_events RENAME COLUMN payload TO broken",
        "ALTER TABLE goals RENAME COLUMN snapshot TO broken",
        "ALTER TABLE goals RENAME COLUMN owner_node TO broken",
        "ALTER TABLE goal_tasks RENAME COLUMN thread_id TO broken",
        "ALTER TABLE thread_bindings ADD CONSTRAINT reject_insert CHECK (FALSE)",
        "ALTER TABLE dispatches ADD CONSTRAINT reject_insert CHECK (FALSE)",
        "CREATE FUNCTION reject_commit() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail_commit AFTER INSERT ON dispatches DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_commit()",
    ] {
        let f = Fixture::new().await;
        let token = ready(&f, 2, 4).await;
        f.sql(fault).await;
        let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
        assert!(
            matches!(
                service.submit(&token, request(1300)).await,
                Err(Error::Database(_))
            ),
            "{fault}"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_bindings")
            .fetch_one(&f.raw)
            .await
            .expect("count");
        assert_eq!(count, 0);
        f.close().await;
    }
}

#[tokio::test]
async fn routing_authentication_and_closed_storage_fail_before_selection() {
    let f = Fixture::enrolled().await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Unconfigured));
    let invalid = "b".repeat(64).try_into().expect("token");
    assert!(matches!(
        service.submit(&invalid, request(1400)).await,
        Err(Error::Unauthorized)
    ));
    assert!(f.store.poll_dispatch(&invalid, &poll()).await.is_err());
    f.sql("ALTER TABLE groups RENAME COLUMN group_id TO broken")
        .await;
    assert!(service.submit(&data::token(), request(1400)).await.is_err());
    f.store.close().await;
    assert!(service.submit(&data::token(), request(1400)).await.is_err());
    assert!(f.store.thread(&data::token(), id(1401)).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn concurrent_routes_reserve_capacity_and_same_request_has_one_assignment() {
    for same in [false, true] {
        let f = Fixture::new().await;
        let token = ready(&f, 2, 1).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let selector = Arc::new(Selection {
            gate: Some((entered, release.clone())),
            ..Selection::default()
        });
        let service = RoutingService::new(f.store.clone(), selector.clone());
        let mut handles = Vec::new();
        for seed in [1500, if same { 1500 } else { 1600 }] {
            let service = service.clone();
            let token = token.clone();
            handles.push(tokio::spawn(async move {
                service.submit(&token, request(seed)).await
            }));
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while selector.contexts.lock().expect("contexts").len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both models in flight");
        release.notify_waiters();
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.expect("join"));
        }
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if same { 2 } else { 1 }
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM dispatches")
            .fetch_one(&f.raw)
            .await
            .expect("count");
        assert_eq!(count, 1);
        f.close().await;
    }
}

#[tokio::test]
async fn claiming_and_failover_never_authorize_two_nodes() {
    for _ in 0..8 {
        let f = Fixture::new().await;
        let a = ready(&f, 2, 1).await;
        let b = ready(&f, 9, 1).await;
        let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
        let req = request(1700);
        service.submit(&a, req.clone()).await.expect("queued");
        f.sql("UPDATE nodes SET dispatch_seen=clock_timestamp()-interval '1 hour' WHERE node_id='00000000-0000-0000-0000-000000000002'").await;
        let inventory = poll();
        let (claimed, moved) = tokio::join!(
            f.store.poll_dispatch(&a, &inventory),
            service.failover(&b, req.thread_id)
        );
        let claimed = claimed.expect("claim transaction");
        let peer = f
            .store
            .poll_dispatch(&b, &poll())
            .await
            .expect("peer claim");
        assert_eq!(
            usize::from(claimed.command.is_some()) + usize::from(peer.command.is_some()),
            1
        );
        if let Some(command) = peer.command {
            assert_eq!(moved.expect("moved").command, command);
        } else {
            assert_eq!(claimed.command.expect("original claim").node_id, id(2));
        }
        f.close().await;
    }
}

#[tokio::test]
async fn another_group_cannot_read_or_continue_a_thread() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let req = request(1800);
    service.submit(&token, req.clone()).await.expect("start");
    let outsider = "f".repeat(64).try_into().expect("token");
    f.store
        .enroll(id(50), id(51), &outsider)
        .await
        .expect("another group");
    assert!(f.store.thread(&outsider, req.thread_id).await.is_err());
    assert!(service.failover(&outsider, req.thread_id).await.is_err());
    let mut follow = req;
    follow.command_id = id(1900);
    follow.action = RouteAction::Cancel {};
    assert!(service.submit(&outsider, follow).await.is_err());
    assert!(
        f.store
            .poll_dispatch(&outsider, &poll())
            .await
            .expect("no private work")
            .command
            .is_none()
    );
    f.close().await;
}
