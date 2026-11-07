#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{
    Backend,
    end_to_end::Database,
    http::{request, sign_in},
    initialize, registration,
};
use axum::http::StatusCode;
use orchestrator_protocol::{
    agent::Message,
    capabilities::{Capability, Platform},
    routing::DispatchPoll,
    sync::NodeProfile,
};
use uuid::Uuid;
fn workspace() -> ProviderWorkspace {
    ProviderWorkspace {
        workspace_id: Uuid::from_u128(5).try_into().expect("id"),
        name: "Test workspace".into(),
        provider: LocalProvider::Codex,
        allow_edits: true,
    }
}
fn installed(_: LocalProvider) -> Option<PathBuf> {
    PathBuf::from("/bin/cat").canonicalize().ok()
}
#[tokio::test]
async fn chat_submission_history_and_workspace_configuration_use_authenticated_routes() {
    let dir = tempfile::tempdir().expect("dir");
    let mut machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(machine.agent_journal().await.is_err());
    initialize(&machine).await;
    machine.provider_path = installed;
    let app = crate::web::router(machine.clone());
    check_unauthenticated_routes(&app).await;
    let cookie = sign_in(&app, &machine).await;
    for path in [
        "/api/local/chat",
        "/api/local/chat/workspaces",
        "/api/local/chat/00000000-0000-0000-0000-000000000001",
        "/api/local/chat/00000000-0000-0000-0000-000000000001/activity",
    ] {
        assert!(
            !request(&app, path, None, Some(&cookie), None)
                .await
                .0
                .is_success()
        );
    }
    assert!(machine.chat_workspaces().await.is_err());
    assert!(
        machine
            .create_workspace_with(workspace(), |_| None)
            .await
            .is_err()
    );
    machine
        .create_workspace_with(workspace(), installed)
        .await
        .expect("configure");
    machine
        .create_workspace_with(workspace(), installed)
        .await
        .expect("repeat");
    assert_eq!(machine.local_workspaces().await.expect("local").len(), 1);
    assert_eq!(
        request(&app, "/api/local/workspaces", None, Some(&cookie), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "/api/local/workspaces",
            Some(json!(workspace())),
            Some(&cookie),
            Some("https://evil.example")
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/api/local/providers", None, Some(&cookie), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "/api/local/workspaces",
            Some(json!(workspace())),
            Some(&cookie),
            Some("http://127.0.0.1:8080")
        )
        .await
        .0,
        StatusCode::OK
    );
    let database = Database::new().await;
    let store = Store::open(&database.url).await.expect("store");
    machine.execution.set(store.clone()).expect("execution");
    assert!(machine.chats().await.expect("empty").is_empty());
    let journal = advertise_workspace(&machine, &store).await;
    check_conversation(&machine, &app, &cookie).await;
    let mut invalid_workspace = workspace();
    invalid_workspace.name.clear();
    assert!(
        !request(
            &app,
            "/api/local/workspaces",
            Some(json!(invalid_workspace)),
            Some(&cookie),
            Some("http://127.0.0.1:8080")
        )
        .await
        .0
        .is_success()
    );
    check_storage_errors(&machine, &app, &cookie, &database.url).await;
    journal.close().await;
    machine.close().await;
    database.close().await;
}
#[tokio::test]
async fn unavailable_provider_and_storage_are_reported_without_leaking_credentials() {
    assert!(
        providers_with(&|_| None, Duration::from_secs(1))
            .await
            .iter()
            .all(|v| v["installed"] == false)
    );
    assert!(
        providers_with(
            &|_| Some("/no/such/provider".into()),
            Duration::from_secs(1)
        )
        .await
        .iter()
        .all(|v| v["signed_in"] == false)
    );
    assert!(
        providers_with(&installed, Duration::from_secs(1))
            .await
            .iter()
            .all(|v| v["signed_in"] == false)
    );
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    let mut registration = registration();
    registration.role = orchestrator_protocol::installation::InstallationRole::Router;
    machine
        .initialize(
            &registration,
            "http://127.0.0.1:8081",
            "http://127.0.0.1:8080",
        )
        .await
        .expect("initialize");
    assert!(machine.agent_journal().await.is_err());
    machine.close().await;
}

async fn check_conversation(machine: &Machine, app: &axum::Router, cookie: &str) {
    let route = RouteRequest {
        command_id: Uuid::new_v4().try_into().expect("id"),
        thread_id: Uuid::new_v4().try_into().expect("id"),
        action: RouteAction::Start {
            task_id: Uuid::new_v4().try_into().expect("id"),
            session_id: Uuid::new_v4().try_into().expect("id"),
            workspace_id: workspace().workspace_id,
            capabilities: [Capability::local_provider(LocalProvider::Codex)].into(),
            message: Message {
                attachments: Vec::new(),
                id: Uuid::new_v4().try_into().expect("id"),
                text: "💡".repeat(5_000).try_into().expect("text"),
            },
        },
    };
    let (status, body, _) = request(
        app,
        "/api/local/chat",
        Some(json!(route)),
        Some(cookie),
        Some("http://127.0.0.1:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = machine.send_chat(route.clone()).await.expect("idempotent");
    assert_eq!(
        machine.chat(route.thread_id).await.expect("history"),
        vec![result]
    );
    for path in [
        "/api/local/chat".to_owned(),
        "/api/local/chat/workspaces".to_owned(),
        format!("/api/local/chat/{}", route.thread_id),
        format!("/api/local/chat/{}/activity", route.thread_id),
    ] {
        assert_eq!(
            request(app, &path, None, Some(cookie), None).await.0,
            StatusCode::OK
        );
    }
    let mut invalid = route.clone();
    if let RouteAction::Start { capabilities, .. } = &mut invalid.action {
        capabilities.clear();
    }
    assert!(machine.send_chat(invalid).await.is_err());
    let mut send = route;
    send.command_id = Uuid::new_v4().try_into().expect("id");
    send.action = RouteAction::Cancel {};
    assert!(
        machine.send_chat(send).await.is_err(),
        "busy thread cannot be reassigned"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sign_in_probe_distinguishes_success_signed_out_and_timeouts() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("provider");
    for (script, expected) in [
        ("echo '{\"loggedIn\":true}'", true),
        ("echo '{\"loggedIn\":false}'", false),
        ("echo broken", false),
    ] {
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
        let status = providers_with(&|_| Some(path.clone()), Duration::from_secs(1)).await;
        assert_eq!(status[0]["signed_in"], true);
        assert_eq!(status[1]["signed_in"], expected);
    }
    std::fs::write(&path, "#!/bin/sh\nsleep 1\n").expect("write");
    assert!(
        providers_with(&|_| Some(path.clone()), Duration::from_millis(10))
            .await
            .iter()
            .all(|v| v["signed_in"] == false)
    );
    let _ = provider_path(LocalProvider::Codex);
    let _ = provider_path(LocalProvider::Claude);
}
#[tokio::test]
async fn chat_storage_failures_and_mismatched_identity_fail_closed() {
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    assert!(machine.chats().await.is_err());
    assert!(
        machine
            .chat(Uuid::new_v4().try_into().expect("id"))
            .await
            .is_err()
    );
    initialize(&machine).await;
    std::fs::write(dir.path().join("agent"), "blocked").expect("block");
    assert!(machine.agent_journal().await.is_err());
    std::fs::remove_file(dir.path().join("agent")).expect("unblock");
    std::fs::write(dir.path().join("workspaces"), "blocked").expect("block");
    assert!(
        machine
            .create_workspace_with(workspace(), installed)
            .await
            .is_err()
    );
    std::fs::remove_file(dir.path().join("workspaces")).expect("unblock");
    std::fs::remove_file(dir.path().join("prompts/execution/specialist.md")).expect("prompt");
    assert!(
        machine
            .create_workspace_with(workspace(), installed)
            .await
            .is_err()
    );
    let database = Database::new().await;
    let store = Store::open(&database.url).await.expect("store");
    machine.execution.set(store.clone()).expect("store");
    store
        .enroll(
            Uuid::from_u128(9).try_into().expect("id"),
            Uuid::from_u128(10).try_into().expect("id"),
            &registration().credential,
        )
        .await
        .expect("conflicting credential");
    assert!(machine.chat_store().await.is_err());
    store.close().await;
    assert!(machine.chat_store().await.is_err());
    machine.close().await;
    database.close().await;
}

async fn advertise_workspace(machine: &Machine, store: &Store) -> Journal {
    let own = registration();
    let profile = NodeProfile {
        platform: Platform::Linux,
        capabilities: [Capability::local_provider(LocalProvider::Codex)].into(),
        capacity: 4,
    };
    let journal = machine.agent_journal().await.expect("journal");
    let report = journal.sync_report(profile).await.expect("report");
    store.sync(&own.credential, &report).await.expect("sync");
    store
        .poll_dispatch(
            &own.credential,
            &DispatchPoll {
                workspaces: vec![workspace().workspace_id],
                active_sessions: 0,
                accepted: vec![],
            },
        )
        .await
        .expect("poll");
    store
        .advertise_workspaces(&own.credential, &[workspace()])
        .await
        .expect("advertise");
    assert_eq!(
        machine.chat_workspaces().await.expect("workspaces").len(),
        1
    );
    journal
}

#[tokio::test]
async fn attachment_http_requires_admin_and_round_trips_exact_content() {
    use orchestrator_protocol::{
        attachments::{Attachment, AttachmentChunk},
        ids::AttachmentId,
    };
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    let app = crate::web::router(machine.clone());
    let cookie = sign_in(&app, &machine).await;
    let file = Attachment {
        id: AttachmentId::random(),
        workspace_id: workspace().workspace_id,
        name: "data.txt".into(),
        media_type: "text/plain".into(),
        size: 3,
        sha256: format!("{:x}", Sha256::digest(b"abc")),
    };
    let chunk = AttachmentChunk {
        attachment: file.clone(),
        index: 0,
        data: "YWJj".into(),
    };
    assert_eq!(
        request(
            &app,
            "/api/local/attachments",
            Some(json!(chunk)),
            None,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(machine.upload_attachment(&chunk).await.is_err());
    assert!(machine.attachment_chunk(file.id, 0).await.is_err());
    let database = Database::new().await;
    machine
        .execution
        .set(Store::open(&database.url).await.expect("store"))
        .expect("cell");
    assert_eq!(
        request(
            &app,
            "/api/local/attachments",
            Some(json!(chunk)),
            Some(&cookie),
            Some("http://127.0.0.1:8080")
        )
        .await
        .0,
        StatusCode::OK
    );
    let url = format!("/api/local/attachments/{}/chunks/0", file.id);
    assert_eq!(
        request(&app, &url, None, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let response = request(&app, &url, None, Some(&cookie), None).await;
    assert_eq!(response.0, StatusCode::OK);
    assert_eq!(response.1["data"], "YWJj");
    let mut broken = chunk;
    broken.data = "wrong".into();
    assert!(
        !request(
            &app,
            "/api/local/attachments",
            Some(json!(broken)),
            Some(&cookie),
            Some("http://127.0.0.1:8080")
        )
        .await
        .0
        .is_success()
    );
    assert!(
        !request(
            &app,
            &format!("/api/local/attachments/{}/chunks/1", file.id),
            None,
            Some(&cookie),
            None
        )
        .await
        .0
        .is_success()
    );
    assert!(machine.upload_attachment(&broken).await.is_err());
    assert!(machine.attachment_chunk(file.id, 1).await.is_err());
    machine.close().await;
    database.close().await;
}

async fn check_storage_errors(machine: &Machine, app: &axum::Router, cookie: &str, url: &str) {
    let pool = sqlx::PgPool::connect(url).await.expect("pool");
    sqlx::query("ALTER TABLE execution_activity RENAME TO hidden_activity")
        .execute(&pool)
        .await
        .expect("fault");
    assert!(
        machine
            .chat_activity(Uuid::from_u128(201).try_into().expect("thread"))
            .await
            .is_err()
    );
    sqlx::query("ALTER TABLE hidden_activity RENAME TO execution_activity")
        .execute(&pool)
        .await
        .expect("restore");

    sqlx::query("ALTER TABLE provider_workspaces RENAME TO hidden_workspaces")
        .execute(&pool)
        .await
        .expect("fault");
    assert!(
        !request(app, "/api/local/chat/workspaces", None, Some(cookie), None)
            .await
            .0
            .is_success()
    );
    let route = RouteRequest {
        command_id: Uuid::new_v4().try_into().expect("id"),
        thread_id: Uuid::new_v4().try_into().expect("id"),
        action: RouteAction::Start {
            task_id: Uuid::new_v4().try_into().expect("id"),
            session_id: Uuid::new_v4().try_into().expect("id"),
            workspace_id: workspace().workspace_id,
            capabilities: [Capability::local_provider(LocalProvider::Codex)].into(),
            message: Message {
                id: Uuid::new_v4().try_into().expect("id"),
                text: "test".to_owned().try_into().expect("text"),
                attachments: vec![],
            },
        },
    };
    assert!(
        !request(
            app,
            "/api/local/chat",
            Some(json!(route)),
            Some(cookie),
            Some("http://127.0.0.1:8080")
        )
        .await
        .0
        .is_success()
    );
    assert_eq!(
        request(app, "/api/local/chat", Some(json!(route)), None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("ALTER TABLE dispatches RENAME TO hidden_dispatches")
        .execute(&pool)
        .await
        .expect("fault");
    for path in [
        "/api/local/chat",
        "/api/local/chat/00000000-0000-0000-0000-000000000001",
        "/api/local/chat/00000000-0000-0000-0000-000000000001/activity",
    ] {
        assert!(
            !request(app, path, None, Some(cookie), None)
                .await
                .0
                .is_success()
        );
    }
    pool.close().await;
    let sqlite = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        machine.directory().join("agent/node.sqlite").display()
    ))
    .await
    .expect("sqlite");
    sqlx::query("ALTER TABLE workspace_providers RENAME TO hidden_providers")
        .execute(&sqlite)
        .await
        .expect("fault");
    assert!(
        !request(app, "/api/local/workspaces", None, Some(cookie), None)
            .await
            .0
            .is_success()
    );
    sqlite.close().await;
}

async fn check_unauthenticated_routes(app: &axum::Router) {
    for path in [
        "/api/local/providers",
        "/api/local/workspaces",
        "/api/local/chat",
        "/api/local/chat/workspaces",
        "/api/local/chat/00000000-0000-0000-0000-000000000001",
        "/api/local/chat/00000000-0000-0000-0000-000000000001/activity",
    ] {
        assert_eq!(
            request(app, path, None, None, None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn workspace_setup_and_chat_fail_before_using_an_uninitialized_agent() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    let prompt = dir.path().join("prompt.md");
    std::fs::write(&prompt, "test").expect("prompt");
    assert!(
        execution_config(
            workspace(),
            PathBuf::from("/bin/cat"),
            &dir.path().join("missing"),
            &prompt
        )
        .is_err()
    );
    assert!(
        machine
            .create_workspace_with(workspace(), installed)
            .await
            .is_err()
    );
    let instructions = dir.path().join("prompts/execution/specialist.md");
    std::fs::create_dir_all(instructions.parent().expect("parent")).expect("prompt directory");
    std::fs::write(&instructions, "test").expect("prompt");
    assert!(
        machine
            .create_workspace_with(workspace(), installed)
            .await
            .is_err()
    );
    assert!(machine.local_workspaces().await.is_err());
    assert!(
        machine
            .send_chat(RouteRequest {
                command_id: Uuid::new_v4().try_into().expect("id"),
                thread_id: Uuid::new_v4().try_into().expect("id"),
                action: RouteAction::Cancel {}
            })
            .await
            .is_err()
    );
    assert!(provider_path_in(LocalProvider::Codex, None, None).is_none());
    assert!(provider_path_in(LocalProvider::Codex, None, Some("invalid:home".into())).is_none());
    machine.close().await;
}
