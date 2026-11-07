use crate::tests::{
    common::{Fixture, Server},
    data,
};
use orchestrator_coordinator::{Error, routing::RoutingService, selector::Selector, web};
use orchestrator_protocol::{
    agent::{AgentAction, AgentEvent, AgentEventKind},
    ids::{NodeId, WorkspaceId},
    routing::{
        DeliveryState, DispatchPoll, DispatchView, RouteAction, RouteRequest, RoutingContext,
    },
    sync::{CommandOutcome, NodeToken, SyncEvent},
};
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub fn id<T: FromStr>(value: u128) -> T
where
    T::Err: Debug,
{
    Uuid::from_u128(value)
        .to_string()
        .parse()
        .expect("identity")
}

pub fn request(seed: u128) -> RouteRequest {
    let report = data::report();
    let AgentAction::Start {
        workspace_id,
        message,
    } = report.events[0].command.action.clone()
    else {
        unreachable!()
    };
    RouteRequest {
        command_id: id(seed),
        thread_id: id(seed + 1),
        action: RouteAction::Start {
            task_id: id(seed + 2),
            session_id: id(seed + 3),
            workspace_id,
            capabilities: report.profile.capabilities,
            message,
        },
    }
}

#[derive(Debug, Default)]
pub struct Selection {
    pub contexts: Mutex<Vec<RoutingContext>>,
    pub destination: Option<NodeId>,
    pub gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
impl Selector for Selection {
    fn choose<'a>(
        &'a self,
        context: &'a RoutingContext,
    ) -> Pin<Box<dyn Future<Output = Result<NodeId, Error>> + Send + 'a>> {
        Box::pin(async move {
            self.contexts.lock().expect("context").push(context.clone());
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(self.destination.unwrap_or(context.candidates[0].node_id))
        })
    }
}

pub fn poll() -> DispatchPoll {
    DispatchPoll {
        workspaces: vec![id::<WorkspaceId>(5)],
        active_sessions: 0,
        accepted: vec![],
    }
}
pub async fn ready(f: &Fixture, node: u128, capacity: u16) -> NodeToken {
    let token: NodeToken = format!("{node:064x}").try_into().expect("token");
    let mut report = data::report();
    report.node_id = id(node);
    report.events.clear();
    report.active_sessions = 0;
    report.profile.capacity = capacity;
    f.store
        .enroll(report.group_id, report.node_id, &token)
        .await
        .expect("enroll");
    f.store.sync(&token, &report).await.expect("profile");
    f.store
        .poll_dispatch(&token, &poll())
        .await
        .expect("readiness");
    token
}

pub(super) async fn finish(f: &Fixture, token: &NodeToken, view: &DispatchView, sequence: i64) {
    let command = &view.command;
    let kind = match &command.action {
        AgentAction::Start { message, .. } | AgentAction::Send { message } => {
            AgentEventKind::Replied {
                message_id: message.id,
                text: message.text.clone(),
            }
        }
        AgentAction::Cancel => AgentEventKind::Cancelled,
    };
    let event = AgentEvent {
        version: command.version,
        command_id: command.command_id,
        group_id: command.group_id,
        node_id: command.node_id,
        target: command.target,
        kind,
    };
    let mut report = data::report();
    report.node_id = command.node_id;
    report.profile = f
        .store
        .nodes(token)
        .await
        .expect("nodes")
        .into_iter()
        .find(|n| n.node_id == command.node_id)
        .expect("node")
        .profile
        .expect("profile");
    report.events = vec![SyncEvent {
        sequence,
        command: command.clone(),
        outcome: CommandOutcome::Completed {
            event: Box::new(event),
        },
    }];
    f.store.sync(token, &report).await.expect("commit result");
}

#[tokio::test]
async fn affinity_survives_replicas_and_restart_requires_confirmed_cancellation() {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 4).await;
    let b = ready(&f, 9, 4).await;
    let selector = Arc::new(Selection::default());
    let service = RoutingService::new(f.store.clone(), selector.clone());
    let req = request(100);
    let start = service.submit(&a, req.clone()).await.expect("start");
    assert_eq!(start.command.node_id, id(2));
    assert_eq!(
        service.submit(&a, req.clone()).await.expect("replay"),
        start
    );
    let mut changed = req.clone();
    changed.thread_id = id(400);
    assert!(matches!(
        service.submit(&a, changed).await,
        Err(Error::RoutingConflict(_))
    ));
    let mut follow = req.clone();
    follow.command_id = id(110);
    follow.action = RouteAction::Send {
        message: match &start.command.action {
            AgentAction::Start { message, .. } => message.clone(),
            _ => unreachable!(),
        },
    };
    assert!(matches!(
        service.submit(&a, follow.clone()).await,
        Err(Error::RoutingConflict(_))
    ));
    assert_eq!(
        f.store
            .poll_dispatch(&a, &poll())
            .await
            .expect("claim")
            .command,
        Some(start.command.clone())
    );
    finish(&f, &a, &start, 1).await;
    f.sql("UPDATE nodes SET dispatch_seen=clock_timestamp()-interval '1 hour' WHERE node_id='00000000-0000-0000-0000-000000000002'").await;
    let replica = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("replica");
    let replacement = RoutingService::new(
        replica.clone(),
        Arc::new(orchestrator_coordinator::selector::Unconfigured),
    );
    let sent = replacement
        .submit(&b, follow.clone())
        .await
        .expect("affinity needs no model");
    assert_eq!(sent.command.target, start.command.target);
    assert_eq!(sent.command.node_id, start.command.node_id);
    assert!(matches!(
        replacement.failover(&b, req.thread_id).await,
        Err(Error::Unresolved)
    ));
    f.store.poll_dispatch(&a, &poll()).await.expect("reconnect");
    finish(&f, &a, &sent, 2).await;
    let mut restart = req.clone();
    restart.command_id = id(112);
    restart.action = RouteAction::Restart {
        session_id: id(500),
        message: match follow.action {
            RouteAction::Send { message } => message,
            _ => unreachable!(),
        },
    };
    assert!(replacement.submit(&b, restart.clone()).await.is_err());
    let mut cancel = req.clone();
    cancel.command_id = id(113);
    cancel.action = RouteAction::Cancel {};
    let cancelled = replacement.submit(&b, cancel).await.expect("cancel");
    f.store
        .poll_dispatch(&a, &poll())
        .await
        .expect("claim cancel");
    finish(&f, &a, &cancelled, 3).await;
    f.store.revoke(id(1), id(2)).await.expect("revoke");
    let restarted = service
        .submit(&b, restart)
        .await
        .expect("confirmed stop permits restart");
    assert_eq!(restarted.command.node_id, id(9));
    assert_eq!(restarted.command.target.session_id, id(500));
    assert_eq!(
        restarted.command.target.execution_epoch,
        start.command.target.execution_epoch.next().expect("epoch")
    );
    assert_eq!(selector.contexts.lock().expect("contexts").len(), 2);
    assert_eq!(
        f.store.thread(&b, req.thread_id).await.expect("read"),
        restarted
    );
    replica.close().await;
    f.close().await;
}

#[tokio::test]
async fn failover_moves_only_unclaimed_starts_and_late_results_cannot_cross_epochs() {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 1).await;
    let b = ready(&f, 9, 1).await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let req = request(200);
    let first = service.submit(&a, req.clone()).await.expect("start");
    assert_eq!(
        service
            .failover(&b, req.thread_id)
            .await
            .expect("healthy owner"),
        first
    );
    f.sql("UPDATE nodes SET dispatch_seen=clock_timestamp()-interval '1 hour' WHERE node_id='00000000-0000-0000-0000-000000000002'").await;
    let moved = service
        .failover(&b, req.thread_id)
        .await
        .expect("unclaimed move");
    assert_eq!(moved.command.node_id, id(9));
    assert_ne!(
        moved.command.target.execution_epoch,
        first.command.target.execution_epoch
    );
    assert!(
        f.store
            .poll_dispatch(&a, &poll())
            .await
            .expect("old node reconnect")
            .command
            .is_none()
    );
    assert_eq!(
        f.store
            .poll_dispatch(&b, &poll())
            .await
            .expect("new claim")
            .command,
        Some(moved.command.clone())
    );
    assert_eq!(
        f.store
            .poll_dispatch(&b, &poll())
            .await
            .expect("lost response")
            .command,
        Some(moved.command.clone())
    );
    assert!(matches!(
        service.failover(&a, req.thread_id).await,
        Err(Error::Unresolved)
    ));
    let mut ack = poll();
    ack.accepted = vec![moved.command.command_id];
    ack.active_sessions = 1;
    assert!(
        f.store
            .poll_dispatch(&b, &ack)
            .await
            .expect("accepted")
            .command
            .is_none()
    );
    assert!(
        f.store
            .poll_dispatch(&b, &ack)
            .await
            .expect("ack replay")
            .command
            .is_none()
    );
    let mut forged = data::report();
    forged.events[0].command = first.command;
    forged.events[0].outcome = CommandOutcome::Rejected {
        reason: "late old result".into(),
    };
    assert!(matches!(
        f.store.sync(&a, &forged).await,
        Err(Error::RoutingConflict(_))
    ));
    finish(&f, &b, &moved, 1).await;
    assert_eq!(
        f.store
            .thread(&a, req.thread_id)
            .await
            .expect("completed")
            .state,
        DeliveryState::Completed
    );
    f.close().await;
}

#[tokio::test]
async fn selection_filters_capacity_mounts_capabilities_and_rechecks_after_model_latency() {
    let f = Fixture::new().await;
    let a = ready(&f, 2, 1).await;
    let b = ready(&f, 9, 1).await;
    let gate = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let selector = Arc::new(Selection {
        gate: Some(gate.clone()),
        ..Selection::default()
    });
    let service = RoutingService::new(f.store.clone(), selector.clone());
    let req = request(300);
    let pending = service.submit(&a, req.clone());
    tokio::pin!(pending);
    tokio::select! {result=&mut pending=>panic!("model returned early: {result:?}"),()=gate.0.notified()=>{}}
    f.store
        .poll_dispatch(
            &a,
            &DispatchPoll {
                workspaces: vec![],
                ..poll()
            },
        )
        .await
        .expect("unmount");
    gate.1.notify_one();
    assert!(matches!(pending.await, Err(Error::NoCapacity)));
    let invalid = RoutingService::new(
        f.store.clone(),
        Arc::new(Selection {
            destination: Some(id(99)),
            ..Selection::default()
        }),
    );
    assert!(matches!(
        invalid.submit(&b, req.clone()).await,
        Err(Error::NoCapacity)
    ));
    let good = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let start = good.submit(&b, req.clone()).await.expect("only B eligible");
    assert_eq!(start.command.node_id, id(9));
    assert!(matches!(
        good.submit(&b, request(400)).await,
        Err(Error::NoCapacity)
    ));
    f.store.poll_dispatch(&b, &poll()).await.expect("claim");
    finish(&f, &b, &start, 1).await;
    // Completed does not mean the stateful specialist released its slot.
    assert!(matches!(
        good.submit(&b, request(400)).await,
        Err(Error::NoCapacity)
    ));
    let mut impossible = request(500);
    if let RouteAction::Start { capabilities, .. } = &mut impossible.action {
        capabilities.insert("build.ios".to_owned().try_into().expect("capability"));
    }
    assert!(matches!(
        good.submit(&b, impossible).await,
        Err(Error::NoCapacity)
    ));
    assert_eq!(
        selector.contexts.lock().expect("contexts")[0]
            .candidates
            .len(),
        2
    );
    f.close().await;
}

#[tokio::test]
async fn authenticated_http_routing_and_delivery_are_group_scoped() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let server = Server::start(web::router_with_selector(
        f.store.clone(),
        Arc::new(Selection::default()),
    ))
    .await;
    let client = reqwest::Client::new();
    let req = request(600);
    assert_eq!(
        client
            .post(format!("{}/v1/tasks", server.origin))
            .json(&req)
            .send()
            .await
            .expect("http")
            .status(),
        401
    );
    let response = client
        .post(format!("{}/v1/tasks", server.origin))
        .bearer_auth(token.expose())
        .json(&req)
        .send()
        .await
        .expect("http");
    assert_eq!(response.status(), 200);
    let saved: DispatchView = response.json().await.expect("view");
    for path in [
        format!("/v1/threads/{}", req.thread_id),
        format!("/v1/threads/{}/failover", req.thread_id),
    ] {
        let builder = if path.ends_with("failover") {
            client.post(format!("{}{path}", server.origin))
        } else {
            client.get(format!("{}{path}", server.origin))
        };
        assert_eq!(
            builder
                .bearer_auth(token.expose())
                .send()
                .await
                .expect("http")
                .json::<DispatchView>()
                .await
                .expect("view"),
            saved
        );
    }
    assert_eq!(
        client
            .post(format!("{}/v1/dispatch/poll", server.origin))
            .bearer_auth(token.expose())
            .json(&poll())
            .send()
            .await
            .expect("poll")
            .status(),
        200
    );
    server.close().await;
    f.close().await;
}

#[tokio::test]
async fn old_cancellation_replay_cannot_stop_a_restarted_session() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 4).await;
    let service = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let req = request(2000);
    let first = service.submit(&token, req.clone()).await.expect("start");
    f.store.poll_dispatch(&token, &poll()).await.expect("claim");
    finish(&f, &token, &first, 1).await;
    let mut cancel = req.clone();
    cancel.command_id = id(2010);
    cancel.action = RouteAction::Cancel {};
    let stopped = service.submit(&token, cancel).await.expect("cancel");
    f.store.poll_dispatch(&token, &poll()).await.expect("claim");
    finish(&f, &token, &stopped, 2).await;
    let RouteAction::Start { message, .. } = req.action.clone() else {
        unreachable!()
    };
    let mut restart = req.clone();
    restart.command_id = id(2011);
    restart.action = RouteAction::Restart {
        session_id: id(2012),
        message: message.clone(),
    };
    let replacement = service.submit(&token, restart).await.expect("restart");
    finish(&f, &token, &stopped, 2).await; // Old immutable result is still valid history.
    f.store
        .poll_dispatch(&token, &poll())
        .await
        .expect("claim replacement");
    finish(&f, &token, &replacement, 3).await;
    let mut follow = req;
    follow.command_id = id(2013);
    follow.action = RouteAction::Send { message };
    let sent = service
        .submit(&token, follow)
        .await
        .expect("new session remains open");
    assert_eq!(sent.command.target.session_id, id(2012));
    f.close().await;
}
