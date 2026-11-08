use crate::tests::{
    agent_tools::prepared,
    routing::{finish, id, poll},
};
use orchestrator_protocol::{
    activity::{ActivityPhase, ExecutionActivity},
    execution::LocalProvider,
    recovery::{Checkpoint, RecoveryAction, RecoveryState},
};
#[tokio::test]
async fn recovery_requires_review_original_node_and_exact_checkpoint_across_restart() {
    let (f, a, b, _, parent) = prepared().await;
    let thread = parent.request.thread_id;
    let inspect = RecoveryAction::Inspect {
        id: id(500),
        command_id: parent.command.command_id,
    };
    assert!(f.store.recover(&a, thread, &inspect).await.is_err());
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    f.store
        .record_activity(
            &a,
            &[ExecutionActivity {
                sequence: 1,
                command_id: parent.command.command_id,
                provider: LocalProvider::Codex,
                phase: ActivityPhase::NeedsAttention,
                at_ms: 100,
                summary: None,
            }],
        )
        .await
        .expect("activity");
    let view = f
        .store
        .recover(&a, thread, &inspect)
        .await
        .expect("inspect");
    assert_eq!(
        f.store.recover(&a, thread, &inspect).await.expect("retry"),
        view
    );
    assert_eq!(
        f.store.recoveries(&a, thread).await.expect("list"),
        vec![view.clone()]
    );
    assert!(
        f.store
            .recovery_job(&b)
            .await
            .expect("other node")
            .is_none()
    );
    let job = f
        .store
        .recovery_job(&a)
        .await
        .expect("job")
        .expect("pending");
    assert_eq!(job.command, parent.command);
    let mut report = view;
    report.state = RecoveryState::Ready;
    report.checkpoint = Some(Checkpoint {
        provider: LocalProvider::Codex,
        session_id: parent.command.target.session_id,
        files: 2,
        progress: Some("Wrote initial files".into()),
        digest: "a".repeat(64),
    });
    assert!(f.store.recovery_report(&b, &report).await.is_err());
    f.store.recovery_report(&a, &report).await.expect("ready");
    f.store.recovery_report(&a, &report).await.expect("retry");
    assert!(
        f.store
            .recover(
                &a,
                thread,
                &RecoveryAction::Continue {
                    id: id(500),
                    digest: "changed".into()
                }
            )
            .await
            .is_err()
    );
    finish_recovery(&f, &a, &parent, report, thread).await;
    f.close().await;
}
async fn finish_recovery(
    f: &crate::tests::common::Fixture,
    a: &orchestrator_protocol::sync::NodeToken,
    parent: &orchestrator_protocol::routing::DispatchView,
    mut report: orchestrator_protocol::recovery::RecoveryView,
    thread: orchestrator_protocol::ids::ThreadId,
) {
    let reopened = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("restart");
    let approve = RecoveryAction::Continue {
        id: id(500),
        digest: "a".repeat(64),
    };
    assert_eq!(
        reopened
            .recover(a, thread, &approve)
            .await
            .expect("approve")
            .state,
        RecoveryState::Approved
    );
    assert_eq!(
        reopened
            .recover(a, thread, &approve)
            .await
            .expect("retry approval")
            .state,
        RecoveryState::Approved
    );
    assert!(
        reopened
            .recover(
                a,
                thread,
                &RecoveryAction::Inspect {
                    id: id(501),
                    command_id: parent.command.command_id
                }
            )
            .await
            .is_err()
    );
    report.state = RecoveryState::Resumed;
    reopened.recovery_report(a, &report).await.expect("resumed");
    assert!(reopened.recovery_job(a).await.expect("done").is_none());
    assert_eq!(
        reopened
            .recover(a, thread, &approve)
            .await
            .expect("retry after resume")
            .state,
        RecoveryState::Resumed
    );
    finish(f, a, parent, 1).await;
    assert!(
        reopened
            .recover(
                a,
                thread,
                &RecoveryAction::Inspect {
                    id: id(502),
                    command_id: parent.command.command_id
                }
            )
            .await
            .is_err()
    );
    reopened.close().await;
}

async fn pending() -> (
    crate::tests::common::Fixture,
    orchestrator_protocol::sync::NodeToken,
    orchestrator_protocol::routing::DispatchView,
    orchestrator_protocol::recovery::RecoveryView,
) {
    let (f, a, _, _, parent) = prepared().await;
    f.store.poll_dispatch(&a, &poll()).await.expect("claim");
    f.store
        .record_activity(
            &a,
            &[ExecutionActivity {
                sequence: 1,
                command_id: parent.command.command_id,
                provider: LocalProvider::Codex,
                phase: ActivityPhase::NeedsAttention,
                at_ms: 100,
                summary: None,
            }],
        )
        .await
        .expect("interrupted");
    let view = f
        .store
        .recover(
            &a,
            parent.request.thread_id,
            &RecoveryAction::Inspect {
                id: id(500),
                command_id: parent.command.command_id,
            },
        )
        .await
        .expect("inspect");
    (f, a, parent, view)
}
fn ready(
    mut view: orchestrator_protocol::recovery::RecoveryView,
    parent: &orchestrator_protocol::routing::DispatchView,
) -> orchestrator_protocol::recovery::RecoveryView {
    view.state = RecoveryState::Ready;
    view.checkpoint = Some(Checkpoint {
        provider: LocalProvider::Codex,
        session_id: parent.command.target.session_id,
        files: 1,
        progress: None,
        digest: "a".repeat(64),
    });
    view
}
#[tokio::test]
async fn invalid_reports_never_grant_permission_and_blocked_tasks_can_be_reinspected() {
    let (f, a, parent, view) = pending().await;
    let report = ready(view.clone(), &parent);
    for bad in [
        {
            let mut v = report.clone();
            v.detail = Some("x".repeat(8193));
            v
        },
        {
            let mut v = report.clone();
            v.checkpoint = None;
            v
        },
        {
            let mut v = report.clone();
            v.checkpoint.as_mut().expect("checkpoint").digest = "z".repeat(64);
            v
        },
        {
            let mut v = report.clone();
            v.checkpoint.as_mut().expect("checkpoint").digest = "a".into();
            v
        },
        {
            let mut v = report.clone();
            v.detail = Some("unexpected".into());
            v
        },
        {
            let mut v = report.clone();
            v.state = RecoveryState::Resumed;
            v
        },
        {
            let mut v = view.clone();
            v.state = RecoveryState::Blocked;
            v
        },
        {
            let mut v = view.clone();
            v.state = RecoveryState::Blocked;
            v.detail = Some(String::new());
            v
        },
        {
            let mut v = view.clone();
            v.state = RecoveryState::Blocked;
            v.detail = Some("x".repeat(1001));
            v
        },
    ] {
        assert!(f.store.recovery_report(&a, &bad).await.is_err());
    }
    check_blocked_review(f, a, parent, report).await;
}
async fn check_blocked_review(
    f: crate::tests::common::Fixture,
    a: orchestrator_protocol::sync::NodeToken,
    parent: orchestrator_protocol::routing::DispatchView,
    report: orchestrator_protocol::recovery::RecoveryView,
) {
    let view = report.clone();
    let mut blocked = report.clone();
    blocked.state = RecoveryState::Blocked;
    blocked.detail = Some("Original process has not stopped".into());
    f.store
        .recovery_report(&a, &blocked)
        .await
        .expect("blocked");
    assert!(
        f.store
            .recover(
                &a,
                parent.request.thread_id,
                &RecoveryAction::Continue {
                    id: view.id,
                    digest: "a".repeat(64)
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.store
            .recover(
                &a,
                id(998),
                &RecoveryAction::Continue {
                    id: view.id,
                    digest: "a".repeat(64)
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.store
            .recover(
                &a,
                parent.request.thread_id,
                &RecoveryAction::Inspect {
                    id: view.id,
                    command_id: id(999)
                }
            )
            .await
            .is_err()
    );
    let next = f
        .store
        .recover(
            &a,
            parent.request.thread_id,
            &RecoveryAction::Inspect {
                id: id(501),
                command_id: parent.command.command_id,
            },
        )
        .await
        .expect("inspect again");
    let report = ready(next, &parent);
    f.store.recovery_report(&a, &report).await.expect("ready");
    finish(&f, &a, &parent, 1).await;
    assert!(
        f.store
            .recover(
                &a,
                parent.request.thread_id,
                &RecoveryAction::Continue {
                    id: report.id,
                    digest: "a".repeat(64)
                }
            )
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn recovery_web_delivery_and_reports_require_node_authentication() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (f, a, parent, view) = pending().await;
    let app = orchestrator_coordinator::web::router(f.store.clone());
    let report = ready(view, &parent);
    for auth in [
        None,
        Some("Bearer bad".into()),
        Some(format!("Bearer {}", a.expose())),
    ] {
        let mut request =
            Request::post("/v1/recovery/report").header("content-type", "application/json");
        if let Some(auth) = &auth {
            request = request.header("authorization", auth);
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&report).expect("json")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            if auth.as_ref().is_some_and(|s| s.ends_with(a.expose())) {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    let response = app
        .oneshot(
            Request::post("/v1/dispatch/poll")
                .header("authorization", format!("Bearer {}", a.expose()))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&poll()).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let batch: orchestrator_protocol::routing::DispatchBatch =
        serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.expect("body"))
            .expect("batch");
    assert!(batch.recovery.is_none());
    f.close().await;
}

#[tokio::test]
async fn recovery_storage_failures_leave_the_previous_review_intact() {
    for fault in [
        "CREATE FUNCTION fail() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failed''; END'; CREATE TRIGGER fail BEFORE INSERT OR UPDATE ON task_recovery FOR EACH ROW EXECUTE FUNCTION fail()",
        "CREATE FUNCTION fail() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failed''; END'; CREATE CONSTRAINT TRIGGER fail AFTER INSERT OR UPDATE ON task_recovery DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail()",
    ] {
        let (f, a, parent, view) = pending().await;
        f.sql(fault).await;
        assert!(
            f.store
                .recovery_report(&a, &ready(view.clone(), &parent))
                .await
                .is_err()
        );
        assert!(
            f.store
                .recover(
                    &a,
                    parent.request.thread_id,
                    &RecoveryAction::Inspect {
                        id: id(501),
                        command_id: parent.command.command_id
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            f.store
                .recoveries(&a, parent.request.thread_id)
                .await
                .expect("original review"),
            vec![view]
        );
        f.close().await;
    }
}

#[tokio::test]
async fn recovery_read_failures_and_unauthorized_tokens_are_explicit() {
    for fault in [
        "ALTER TABLE task_recovery RENAME TO hidden",
        "ALTER TABLE dispatches RENAME TO hidden",
        "ALTER TABLE execution_activity RENAME TO hidden",
    ] {
        let (f, a, parent, view) = pending().await;
        let thread = parent.request.thread_id;
        f.sql(fault).await;
        let reviews = f.store.recoveries(&a, thread).await;
        let job = f.store.recovery_job(&a).await;
        if fault.contains("execution_activity") {
            assert_eq!(reviews.expect("saved review"), vec![view.clone()]);
            assert!(job.expect("saved job").is_some());
        } else {
            assert!(reviews.is_err());
            assert!(job.is_err());
        }
        assert!(
            f.store
                .recover(
                    &a,
                    thread,
                    &RecoveryAction::Inspect {
                        id: id(501),
                        command_id: parent.command.command_id
                    }
                )
                .await
                .is_err()
        );
        if !fault.contains("execution_activity") {
            assert!(
                f.store
                    .recovery_report(&a, &ready(view, &parent))
                    .await
                    .is_err()
            );
        }
        f.close().await;
    }
    let (f, a, parent, view) = pending().await;
    f.store.close().await;
    assert!(
        f.store
            .recoveries(&a, parent.request.thread_id)
            .await
            .is_err()
    );
    assert!(f.store.recovery_job(&a).await.is_err());
    assert!(f.store.recovery_report(&a, &view).await.is_err());
    assert!(
        f.store
            .recover(
                &a,
                parent.request.thread_id,
                &RecoveryAction::Inspect {
                    id: id(501),
                    command_id: parent.command.command_id
                }
            )
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn resumed_activity_allows_followups_in_the_same_ordered_conversation() {
    use orchestrator_coordinator::routing::RoutingService;
    use orchestrator_protocol::routing::RouteAction;
    let (f, a, parent, _) = pending().await;
    let service = RoutingService::new(
        f.store.clone(),
        std::sync::Arc::new(crate::tests::routing::Selection::default()),
    );
    let mut next = parent.request.clone();
    next.command_id = id(800);
    let RouteAction::Start { message, .. } = parent.request.action.clone() else {
        unreachable!()
    };
    next.action = RouteAction::Send { message };
    assert!(service.submit(&a, next.clone()).await.is_err());
    f.store
        .record_activity(
            &a,
            &[ExecutionActivity {
                sequence: 2,
                command_id: parent.command.command_id,
                provider: LocalProvider::Codex,
                phase: ActivityPhase::Preparing,
                at_ms: 101,
                summary: None,
            }],
        )
        .await
        .expect("resumed activity");
    let queued = service.submit(&a, next).await.expect("followup accepted");
    assert_eq!(queued.command.target, parent.command.target);
    assert_eq!(queued.command.node_id, parent.command.node_id);
    let mut acknowledged = poll();
    acknowledged.accepted = vec![parent.command.command_id];
    assert!(
        f.store
            .poll_dispatch(&a, &acknowledged)
            .await
            .expect("wait for recovery")
            .command
            .is_none()
    );
    finish(&f, &a, &parent, 1).await;
    assert_eq!(
        f.store
            .poll_dispatch(&a, &poll())
            .await
            .expect("next")
            .command
            .expect("followup")
            .command_id,
        id(800)
    );
    f.close().await;
}

#[tokio::test]
async fn recovery_http_errors_do_not_claim_successful_delivery() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let (f, a, parent, mut view) = pending().await;
    let app = orchestrator_coordinator::web::router(f.store.clone());
    view.state = RecoveryState::Resumed;
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/recovery/report")
                .header("authorization", format!("Bearer {}", a.expose()))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&view).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(!response.status().is_success());
    assert_eq!(
        f.store
            .recoveries(&a, parent.request.thread_id)
            .await
            .expect("saved")[0]
            .state,
        RecoveryState::Inspecting
    );
    f.sql("ALTER TABLE task_recovery RENAME TO hidden").await;
    let response = app
        .oneshot(
            Request::post("/v1/dispatch/poll?recovery=true")
                .header("authorization", format!("Bearer {}", a.expose()))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&poll()).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(!response.status().is_success());
    f.close().await;
}

async fn recovery_node(
    parent: &orchestrator_protocol::routing::DispatchView,
) -> (
    tempfile::TempDir,
    orchestrator_node::runtime::NodeRuntime<crate::tests::delivery::DeliveryLauncher>,
    crate::tests::delivery::DeliveryLauncher,
) {
    use orchestrator_protocol::{
        capabilities::Capability, execution::ProviderWorkspace, permission::ProviderBinding,
    };
    let (temp, mut node, launcher) = crate::tests::delivery::runtime().await;
    let directory = temp.path().canonicalize().expect("root").join("work");
    std::fs::create_dir(&directory).expect("work");
    node.journal()
        .mount(id(5), &directory)
        .await
        .expect("mount");
    let instructions = temp.path().join("prompt.md");
    std::fs::write(&instructions, "Test recovery").expect("prompt");
    node.journal()
        .configure_execution(&orchestrator_node::execution::ExecutionConfig {
            workspace: ProviderWorkspace {
                workspace_id: id(5),
                name: "Recovery".into(),
                provider: LocalProvider::Codex,
                allow_edits: true,
            },
            executable: std::env::current_exe().expect("exe"),
            directory,
            instructions,
        })
        .await
        .expect("configure");
    node.submit(&parent.command).await.expect("accept");
    let instance = launcher.launches.lock().expect("launch")[0]
        .instance_id
        .clone();
    let session = parent.command.target.session_id;
    node.journal()
        .register(session, &instance, 42)
        .await
        .expect("register");
    node.journal()
        .claim(session, &instance)
        .await
        .expect("claim");
    node.journal()
        .bind_provider(
            parent.command.command_id,
            &instance,
            &ProviderBinding {
                group_id: parent.command.group_id,
                node_id: parent.command.node_id,
                target: parent.command.target,
                provider: Capability::local_provider(LocalProvider::Codex),
                provider_session_id: "saved-session".to_owned().try_into().expect("session"),
                resumable: true,
            },
        )
        .await
        .expect("binding");
    let raw = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(node.journal().path()),
    )
    .await
    .expect("journal");
    sqlx::query("INSERT INTO provider_quiescence(command_id,stopped) VALUES(?,1)")
        .bind(parent.command.command_id.to_string())
        .execute(&raw)
        .await
        .expect("stopped fixture");
    raw.close().await;
    node.journal()
        .interrupted(session, &instance, "provider exited")
        .await
        .expect("interruption");
    (temp, node, launcher)
}
#[tokio::test]
async fn coordinator_and_node_recover_together_across_failed_delivery_and_restart() {
    use crate::tests::delivery::DeliveryLauncher;
    use orchestrator_node::{runtime::NodeRuntime, sync::Remote};
    let (f, a, parent, view) = pending().await;
    let (temp, mut node, launcher) = recovery_node(&parent).await;
    let server = lossy_recovery_server(f.store.clone()).await;
    let remote = Remote::new(&server.origin, &a).expect("remote");
    f.sql("UPDATE task_recovery SET view=jsonb_set(view,'{command_id}','\"00000000-0000-0000-0000-000000000999\"')").await;
    assert!(
        remote.dispatch(&mut node).await.is_err(),
        "corrupt recovery identity rejected"
    );
    sqlx::query("UPDATE task_recovery SET view=$1")
        .bind(sqlx::types::Json(&view))
        .execute(&f.raw)
        .await
        .expect("restore");
    assert!(
        remote.dispatch(&mut node).await.is_err(),
        "lost inspection acknowledgment"
    );
    let ready = f
        .store
        .recoveries(&a, parent.request.thread_id)
        .await
        .expect("review")
        .remove(0);
    assert_eq!(ready.state, RecoveryState::Ready);
    f.store
        .recover(
            &a,
            parent.request.thread_id,
            &RecoveryAction::Continue {
                id: view.id,
                digest: ready.checkpoint.expect("checkpoint").digest,
            },
        )
        .await
        .expect("approve");
    let identity = node.journal().identity();
    drop(node);
    let mut node = NodeRuntime::open(
        temp.path(),
        identity,
        DeliveryLauncher {
            fail: true,
            ..launcher.clone()
        },
    )
    .await
    .expect("restart");
    assert!(
        remote.dispatch(&mut node).await.is_err(),
        "spawn failure keeps approval"
    );
    drop(node);
    let mut node = NodeRuntime::open(temp.path(), identity, launcher.clone())
        .await
        .expect("restart again");
    f.sql("CREATE FUNCTION fail_resume() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''response unavailable''; END'; CREATE TRIGGER fail_resume BEFORE UPDATE ON task_recovery FOR EACH ROW EXECUTE FUNCTION fail_resume()").await;
    assert!(remote.dispatch(&mut node).await.is_err());
    assert!(remote.dispatch(&mut node).await.is_err());
    assert_eq!(
        launcher.launches.lock().expect("launches").len(),
        3,
        "one initial, one failed, one successful continuation"
    );
    f.sql("DROP TRIGGER fail_resume ON task_recovery").await;
    remote
        .dispatch(&mut node)
        .await
        .expect("acknowledge saved continuation");
    assert_eq!(
        f.store
            .recoveries(&a, parent.request.thread_id)
            .await
            .expect("saved")[0]
            .state,
        RecoveryState::Resumed
    );
    assert_eq!(
        launcher.launches.lock().expect("launches")[2].command,
        parent.command
    );
    server.close().await;
    f.close().await;
}

async fn lossy_recovery_server(
    store: orchestrator_coordinator::store::Store,
) -> crate::tests::common::Server {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let bad_ack = Arc::new(AtomicBool::new(true));
    let flag = bad_ack.clone();
    let app = orchestrator_coordinator::web::router(store).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let corrupt =
                req.uri().path() == "/v1/recovery/report" && flag.swap(false, Ordering::SeqCst);
            async move {
                let response = next.run(req).await;
                if corrupt {
                    axum::response::IntoResponse::into_response(axum::Json(id::<
                        orchestrator_protocol::ids::CommandId,
                    >(
                        999
                    )))
                } else {
                    response
                }
            }
        },
    ));
    crate::tests::common::Server::start(app).await
}

#[tokio::test]
async fn older_nodes_receive_ordinary_dispatch_without_unsupported_recovery_fields() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (f, a, _, _) = pending().await;
    let app = orchestrator_coordinator::web::router(f.store.clone());
    for path in ["/v1/dispatch/poll", "/v1/dispatch/poll?recovery=false"] {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("authorization", format!("Bearer {}", a.expose()))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&poll()).expect("poll")))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(response.status().is_success());
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.expect("body"))
                .expect("json");
        assert!(value.get("recovery").is_none());
        assert!(value.get("command").is_some());
    }
    assert!(f.store.recovery_job(&a).await.expect("queued").is_some());
    f.close().await;
}
