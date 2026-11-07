use crate::tests::{
    common::{Fixture, Server},
    routing::{Selection, id, ready, request},
};
use orchestrator_coordinator::{routing::RoutingService, web};
use orchestrator_protocol::{
    execution::{LocalProvider, ProviderWorkspace},
    routing::RouteAction,
};
use std::sync::Arc;
#[tokio::test]
async fn workspaces_and_chat_are_persistent_authenticated_and_tenant_scoped() {
    let f = Fixture::new().await;
    let token = ready(&f, 2, 5).await;
    let workspace = ProviderWorkspace {
        workspace_id: id(5),
        name: "Code workspace".into(),
        provider: LocalProvider::Codex,
        allow_edits: true,
    };
    f.store
        .advertise_workspaces(&token, std::slice::from_ref(&workspace))
        .await
        .expect("advertise");
    f.store
        .advertise_workspaces(&token, std::slice::from_ref(&workspace))
        .await
        .expect("idempotent");
    let other = ready(&f, 9, 5).await;
    assert!(
        f.store
            .advertise_workspaces(&other, std::slice::from_ref(&workspace))
            .await
            .is_err()
    );
    let outsider = "f".repeat(64).try_into().expect("token");
    f.store
        .enroll(id(90), id(91), &outsider)
        .await
        .expect("other tenant");
    assert!(
        f.store
            .provider_workspaces(&outsider)
            .await
            .expect("scoped")
            .is_empty()
    );
    assert_eq!(
        f.store
            .provider_workspaces(&token)
            .await
            .expect("list")
            .len(),
        1
    );
    let routing = RoutingService::new(f.store.clone(), Arc::new(Selection::default()));
    let request = request(200);
    let view = routing
        .submit(&token, request.clone())
        .await
        .expect("submit");
    assert_eq!(
        f.store
            .conversation(&token, request.thread_id)
            .await
            .expect("chat"),
        vec![view]
    );
    assert_eq!(f.store.conversations(&token).await.expect("list").len(), 1);
    assert!(
        f.store
            .conversations(&outsider)
            .await
            .expect("scoped")
            .is_empty()
    );
    assert!(
        f.store
            .conversation(&outsider, request.thread_id)
            .await
            .expect("scoped")
            .is_empty()
    );
    let reopened = orchestrator_coordinator::store::Store::open(&f.url)
        .await
        .expect("restart");
    assert_eq!(
        reopened
            .conversations(&token)
            .await
            .expect("persisted")
            .len(),
        1
    );
    reopened.close().await;
    check_workspace_http(&f, &token, &workspace).await;
    // Corrupt history must fail closed rather than manufacture a successful reply.
    f.sql("UPDATE dispatches SET request='{}'::jsonb").await;
    assert!(
        f.store
            .conversation(&token, request.thread_id)
            .await
            .is_err()
    );
    let bad = "c".repeat(64).try_into().expect("token");
    assert!(f.store.provider_workspaces(&bad).await.is_err());
    assert!(f.store.conversations(&bad).await.is_err());
    assert!(f.store.conversation(&bad, request.thread_id).await.is_err());
    assert!(f.store.advertise_workspaces(&bad, &[]).await.is_err());
    if let RouteAction::Start { workspace_id, .. } = request.action {
        assert_eq!(workspace_id, workspace.workspace_id);
    }
    f.close().await;
}

async fn check_workspace_http(
    f: &Fixture,
    token: &orchestrator_protocol::sync::NodeToken,
    workspace: &ProviderWorkspace,
) {
    for name in [String::new(), "x".repeat(129), "Changed".into()] {
        let mut invalid = workspace.clone();
        invalid.name = name;
        assert!(
            f.store
                .advertise_workspaces(token, &[invalid])
                .await
                .is_err()
        );
    }
    assert!(
        f.store
            .advertise_workspaces(token, &vec![workspace.clone(); 101])
            .await
            .is_err()
    );
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    for method in [reqwest::Method::GET, reqwest::Method::POST] {
        let url = format!("{}/v1/workspaces", server.origin);
        assert_eq!(
            client
                .request(method.clone(), &url)
                .json(&vec![workspace.clone()])
                .send()
                .await
                .expect("HTTP")
                .status(),
            401
        );
        assert!(
            client
                .request(method, &url)
                .bearer_auth(token.expose())
                .json(&vec![workspace.clone()])
                .send()
                .await
                .expect("HTTP")
                .status()
                .is_success()
        );
    }
}

#[tokio::test]
async fn chat_directory_storage_failures_roll_back_and_never_bypass_authentication() {
    for fault in [
        "ALTER TABLE provider_workspaces RENAME COLUMN workspace TO broken",
        "CREATE FUNCTION reject_workspace() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''write failure''; END'; CREATE TRIGGER fail_write BEFORE INSERT ON provider_workspaces FOR EACH ROW EXECUTE FUNCTION reject_workspace()",
        "CREATE FUNCTION reject_workspace() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RAISE EXCEPTION ''commit failure''; END'; CREATE CONSTRAINT TRIGGER fail_write AFTER INSERT ON provider_workspaces DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_workspace()",
    ] {
        let f = Fixture::new().await;
        let token = ready(&f, 2, 5).await;
        f.sql(fault).await;
        assert!(
            f.store
                .advertise_workspaces(
                    &token,
                    &[ProviderWorkspace {
                        workspace_id: id(5),
                        name: "Code".into(),
                        provider: LocalProvider::Codex,
                        allow_edits: false
                    }]
                )
                .await
                .is_err()
        );
        f.close().await;
    }
    let f = Fixture::new().await;
    let token = ready(&f, 2, 5).await;
    f.sql("DROP TABLE provider_workspaces").await;
    assert!(f.store.provider_workspaces(&token).await.is_err());
    let server = Server::start(web::router(f.store.clone())).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{}/v1/workspaces", server.origin))
            .bearer_auth(token.expose())
            .send()
            .await
            .expect("response")
            .status(),
        503
    );
    assert_eq!(
        client
            .post(format!("{}/v1/workspaces", server.origin))
            .bearer_auth(token.expose())
            .json(&vec![ProviderWorkspace {
                workspace_id: id(5),
                name: "Code".into(),
                provider: LocalProvider::Codex,
                allow_edits: false
            }])
            .send()
            .await
            .expect("response")
            .status(),
        503
    );
    f.sql("ALTER TABLE dispatches RENAME COLUMN sequence TO broken")
        .await;
    assert!(f.store.conversations(&token).await.is_err());
    assert!(f.store.conversation(&token, id(9)).await.is_err());
    f.store.close().await;
    assert!(f.store.advertise_workspaces(&token, &[]).await.is_err());
    f.close().await;
}
