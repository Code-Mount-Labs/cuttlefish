use crate::tests::{
    common::{Fixture, Server},
    data,
    routing::{Selection, id, request},
};
use orchestrator_coordinator::{routing::RoutingService, web};
use orchestrator_node::{
    journal::Journal,
    model::{LaunchSpec, NodeIdentity},
    process::{Launcher, ManagedProcess},
    runtime::NodeRuntime,
    sync::Remote,
    worker,
};
use orchestrator_protocol::{agent::AgentAction, routing::RouteAction};
use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Default, Clone)]
pub struct DeliveryLauncher {
    pub launches: Arc<Mutex<Vec<LaunchSpec>>>,
    pub fail: bool,
}
#[derive(Debug)]
pub struct DeliveryProcess;
impl ManagedProcess for DeliveryProcess {
    fn try_wait(&mut self) -> io::Result<Option<bool>> {
        Ok(None)
    }
}
impl Launcher for DeliveryLauncher {
    type Process = DeliveryProcess;
    fn spawn(&mut self, _: &Path, spec: &LaunchSpec) -> io::Result<DeliveryProcess> {
        self.launches.lock().expect("launches").push(spec.clone());
        if self.fail {
            return Err(io::Error::other("injected spawn failure"));
        }
        Ok(DeliveryProcess)
    }
}

pub async fn runtime() -> (
    tempfile::TempDir,
    NodeRuntime<DeliveryLauncher>,
    DeliveryLauncher,
) {
    let temp = tempfile::tempdir().expect("temp");
    let launcher = DeliveryLauncher::default();
    let node = NodeRuntime::open(
        temp.path(),
        NodeIdentity {
            group_id: id(1),
            node_id: id(2),
        },
        launcher.clone(),
    )
    .await
    .expect("runtime");
    node.journal()
        .mount(id(5), temp.path())
        .await
        .expect("mount");
    (temp, node, launcher)
}

async fn await_result(journal: &Journal) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !journal.pending_events().await.expect("events").is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("result deadline");
}

fn follow_up(
    mut request: orchestrator_protocol::routing::RouteRequest,
    action: AgentAction,
) -> orchestrator_protocol::routing::RouteRequest {
    request.command_id = id(710);
    let AgentAction::Start { mut message, .. } = action else {
        unreachable!()
    };
    message.id = id(711);
    message.text = "follow up".to_owned().try_into().expect("text");
    request.action = RouteAction::Send { message };
    request
}

#[tokio::test]
async fn runtime_permission_waits_cannot_publish_completion_or_authorize_failover() {
    use orchestrator_protocol::sync::CommandOutcome;
    let f = Fixture::enrolled().await;
    let selector = Arc::new(Selection::default());
    let server = Server::start(web::router_with_selector(f.store.clone(), selector.clone())).await;
    let service = RoutingService::new(f.store.clone(), selector);
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let (_temp, mut node, launcher) = runtime().await;
    remote
        .cycle(&mut node, &data::report().profile)
        .await
        .expect("advertise");
    let req = request(700);
    service
        .submit(&data::token(), req.clone())
        .await
        .expect("route");
    remote.dispatch(&mut node).await.expect("receive");
    let spec = launcher.launches.lock().expect("launches")[0].clone();
    let journal = node.journal();
    let permission = wait_for_permission(journal, &spec).await;
    let CommandOutcome::Completed { mut event } = data::report().events.remove(0).outcome else {
        unreachable!()
    };
    event.command_id = permission.command_id;
    event.target = permission.binding.target;
    let outcome = CommandOutcome::Completed { event };
    assert!(
        journal
            .complete(permission.command_id, &spec.instance_id, &outcome)
            .await
            .is_err()
    );
    assert!(
        journal
            .pending_events()
            .await
            .expect("no outcome")
            .is_empty()
    );
    remote
        .synchronize(journal, &data::report().profile)
        .await
        .expect("sync presence");
    assert!(
        service
            .failover(&data::token(), req.thread_id)
            .await
            .is_err()
    );
    assert!(
        f.store
            .thread(&data::token(), req.thread_id)
            .await
            .expect("thread")
            .outcome
            .is_none()
    );
    // Missing permission storage is also a hard failure, not permission to complete.
    let raw = sqlx::SqlitePool::connect(&format!("sqlite://{}", journal.path().display()))
        .await
        .expect("test connection");
    sqlx::query("DROP TABLE runtime_permissions")
        .execute(&raw)
        .await
        .expect("storage fault");
    assert!(
        journal
            .complete(permission.command_id, &spec.instance_id, &outcome)
            .await
            .is_err()
    );
    assert!(
        journal
            .pending_events()
            .await
            .expect("no fabricated result")
            .is_empty()
    );
    raw.close().await;
    journal.close().await;
    server.close().await;
    f.close().await;
}

async fn wait_for_permission(
    journal: &Journal,
    spec: &LaunchSpec,
) -> orchestrator_protocol::permission::PermissionRequest {
    use orchestrator_protocol::permission::{PermissionRequest, ProviderBinding};
    journal
        .register(spec.command.target.session_id, &spec.instance_id, 42)
        .await
        .expect("register");
    journal
        .claim(spec.command.target.session_id, &spec.instance_id)
        .await
        .expect("claim");
    let permission = PermissionRequest {
        permission_id: id(800),
        command_id: spec.command.command_id,
        binding: ProviderBinding {
            group_id: id(1),
            node_id: id(2),
            target: spec.command.target,
            provider: "test.provider".to_owned().try_into().expect("provider"),
            provider_session_id: "original-provider-session"
                .to_owned()
                .try_into()
                .expect("session"),
            resumable: true,
        },
        provider_request_id: "original-callback".to_owned().try_into().expect("callback"),
        action: "write src/main.rs".to_owned().try_into().expect("action"),
        expires_at_ms: 4_000_000_000_000,
    };
    journal
        .bind_provider(
            permission.command_id,
            &spec.instance_id,
            &permission.binding,
        )
        .await
        .expect("binding");
    journal
        .request_permission(&spec.instance_id, &permission)
        .await
        .expect("permission");
    permission
}

#[tokio::test]
async fn routed_specialist_keeps_its_session_through_supervisor_replacement() {
    let f = Fixture::enrolled().await;
    let selector = Arc::new(Selection::default());
    let server = Server::start(web::router_with_selector(f.store.clone(), selector.clone())).await;
    let service = RoutingService::new(f.store.clone(), selector.clone());
    let remote = Remote::new(&server.origin, &data::token()).expect("remote");
    let (temp, mut node, launcher) = runtime().await;
    remote
        .cycle(&mut node, &data::report().profile)
        .await
        .expect("advertise");
    let req = request(700);
    let start = service
        .submit(&data::token(), req.clone())
        .await
        .expect("route");
    remote.dispatch(&mut node).await.expect("receive");
    let spec = launcher.launches.lock().expect("launches")[0].clone();
    let journal = Journal::attach(node.journal().path())
        .await
        .expect("worker attach");
    let worker_task = tokio::spawn(async move {
        worker::run(
            &journal,
            spec.command.target.session_id,
            &spec.instance_id,
            orchestrator_agents::fake::FakeAgent::new(id(1), id(2)),
        )
        .await
        .expect("worker");
        journal.close().await;
    });
    await_result(node.journal()).await;
    let original = node
        .journal()
        .session(start.command.target.session_id)
        .await
        .expect("session")
        .expect("record");
    node.journal().close().await;
    drop(node);
    let mut replacement = NodeRuntime::open(
        temp.path(),
        NodeIdentity {
            group_id: id(1),
            node_id: id(2),
        },
        launcher.clone(),
    )
    .await
    .expect("replacement");
    remote
        .cycle(&mut replacement, &data::report().profile)
        .await
        .expect("sync and ack");
    let follow = follow_up(req.clone(), start.command.action);
    let continued = service
        .submit(&data::token(), follow)
        .await
        .expect("follow up");
    assert_eq!(continued.command.target, start.command.target);
    remote
        .dispatch(&mut replacement)
        .await
        .expect("deliver follow up");
    await_result(replacement.journal()).await;
    let current = replacement
        .journal()
        .session(continued.command.target.session_id)
        .await
        .expect("session")
        .expect("record");
    assert_eq!(original.instance_id, current.instance_id);
    remote
        .cycle(&mut replacement, &data::report().profile)
        .await
        .expect("result and ack");
    let mut cancel = req;
    cancel.command_id = id(712);
    cancel.action = RouteAction::Cancel {};
    service
        .submit(&data::token(), cancel)
        .await
        .expect("cancel");
    remote
        .dispatch(&mut replacement)
        .await
        .expect("cancel delivery");
    worker_task.await.expect("worker stops");
    remote
        .cycle(&mut replacement, &data::report().profile)
        .await
        .expect("stopped sync");
    assert_eq!(launcher.launches.lock().expect("launches").len(), 1);
    assert_eq!(selector.contexts.lock().expect("selections").len(), 1);
    replacement.journal().close().await;
    server.close().await;
    f.close().await;
}
