use super::*;
use crate::registry::Client;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
async fn server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    (origin, task)
}
#[tokio::test]
async fn client_rejects_unsafe_paths_redirects_oversized_and_malformed_responses() {
    let app = Router::new()
        .route(
            "/redirect",
            post(|| async { axum::response::Redirect::temporary("https://example.com") }),
        )
        .route("/huge", post(|| async { "x".repeat(65_537) }))
        .route("/bad", post(|| async { "not JSON" }))
        .route("/good", post(|| async { Json(json!({"ok":true})) }));
    let (origin, task) = server(app).await;
    let client = Client::new(&origin).expect("client");
    for path in [
        "",
        "//example.com",
        "/bad\\path",
        "/bad\npath",
        "/redirect",
        "/huge",
        "/bad",
        "/missing",
    ] {
        assert!(
            client
                .post::<_, Value>(path, None, &json!({}))
                .await
                .is_err(),
            "{path}"
        );
    }
    assert_eq!(
        client
            .post::<_, Value>("/good", Some(&registration().credential), &json!({}))
            .await
            .expect("good"),
        json!({"ok":true})
    );
    task.abort();
    assert!(
        client
            .post::<_, Value>("/good", None, &json!({}))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn registration_retries_exact_identity_and_advertises_without_database_credentials() {
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let input_seen = seen.clone();
    let app=Router::new().route("/api/v1/installations",post(move|Json(value):Json<Value>|{let seen=input_seen.clone();async move {seen.lock().expect("seen").push(value.clone());Json(json!({"group_id":value["group_id"],"installation_id":value["installation_id"],"router_url":"http://127.0.0.1:8080"}))}}))
        .route("/api/v1/installations/heartbeat",post(|headers:axum::http::HeaderMap,Json(value):Json<Value>|async move {assert!(headers.contains_key("authorization"));assert!(!value.to_string().contains("password"));Json(json!({"ok":true}))}))
        .route("/api/v1/installations/preflight",post(||async{Json(json!({"roles":["router_agent","router"],"router_url":null}))}))
        .route("/api/v1/router/authorize-agent",post(||async{Json(json!({"group_id":registration().group_id}))}));
    let (origin, task) = server(app).await;
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    machine
        .initialize(&registration(), &origin, "http://127.0.0.1:8080")
        .await
        .expect("identity");
    assert!(machine.router_origin().await.is_err());
    let topology = Client::new(&origin)
        .expect("client")
        .preflight(registration().group_id, &registration().invitation)
        .await
        .expect("topology");
    assert_eq!(topology.roles.len(), 2);
    machine
        .advertise(false)
        .await
        .expect("initial registration");
    machine.register().await.expect("retry");
    machine.advertise(true).await.expect("heartbeat");
    assert_eq!(
        machine.router_origin().await.expect("router"),
        "http://127.0.0.1:8080"
    );
    assert_eq!(seen.lock().expect("seen").len(), 2);
    {
        let seen = seen.lock().expect("seen");
        assert_eq!(seen[0], seen[1]);
    }
    assert_eq!(
        machine
            .authorize_agent(registration().installation_id, &registration().credential)
            .await
            .expect("authorization"),
        registration().group_id
    );
    task.abort();
    machine.close().await;
}

#[tokio::test]
async fn directory_mismatches_and_failed_local_commits_never_complete_registration() {
    let expected = registration();
    for reply in [
        json!({"group_id":Uuid::from_u128(99),"installation_id":expected.installation_id,"router_url":"http://127.0.0.1:8080"}),
        json!({"group_id":expected.group_id,"installation_id":Uuid::from_u128(99),"router_url":"http://127.0.0.1:8080"}),
        json!({"group_id":expected.group_id,"installation_id":expected.installation_id,"router_url":"invalid"}),
    ] {
        let (origin, task) = server(Router::new().route(
            "/api/v1/installations",
            post(move || {
                let reply = reply.clone();
                async move { Json(reply) }
            }),
        ))
        .await;
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
            .await
            .expect("machine");
        machine
            .initialize(&expected, &origin, "http://127.0.0.1:8080")
            .await
            .expect("initialize");
        assert!(machine.register().await.is_err());
        assert!(machine.setting("registered").await.expect("flag").is_none());
        machine.close().await;
        task.abort();
    }
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    sqlx::query("UPDATE installation SET platform_origin='invalid'")
        .execute(&machine.pool)
        .await
        .expect("corrupt origin");
    assert!(machine.register().await.is_err());
    machine
        .save_setting("registered", "true")
        .await
        .expect("registered");
    assert!(machine.advertise(false).await.is_err());
    assert!(
        machine
            .authorize_agent(expected.installation_id, &expected.credential)
            .await
            .is_err()
    );
    sqlx::query("DROP TABLE resources")
        .execute(&machine.pool)
        .await
        .expect("corrupt resources");
    assert!(machine.advertise(false).await.is_err());
    sqlx::query("DELETE FROM installation")
        .execute(&machine.pool)
        .await
        .expect("missing identity");
    assert!(machine.advertise(false).await.is_err());
    machine.close().await;
}

#[tokio::test]
async fn authorization_rejects_wrong_group_and_agent_only_installations() {
    let (origin, task) = server(Router::new().route(
        "/api/v1/router/authorize-agent",
        post(|| async { Json(json!({"group_id":Uuid::from_u128(999)})) }),
    ))
    .await;
    for role in [InstallationRole::Router, InstallationRole::Agent] {
        let dir = tempfile::tempdir().expect("directory");
        let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
            .await
            .expect("machine");
        let mut identity = registration();
        identity.role = role;
        machine
            .initialize(&identity, &origin, "http://127.0.0.1:8080")
            .await
            .expect("identity");
        assert!(matches!(
            machine
                .authorize_agent(identity.installation_id, &identity.credential)
                .await,
            Err(Error::Unauthorized)
        ));
        machine.close().await;
    }
    task.abort();
}

#[tokio::test]
async fn registration_never_claims_success_when_router_address_cannot_be_saved() {
    let app=Router::new().route("/api/v1/installations",post(||async {let identity=registration();Json(json!({"group_id":identity.group_id,"installation_id":identity.installation_id,"router_url":"http://127.0.0.1:8080"}))}));
    let (origin, task) = server(app).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    machine
        .initialize(&registration(), &origin, "http://127.0.0.1:8080")
        .await
        .expect("identity");
    sqlx::raw_sql("CREATE TRIGGER fail BEFORE INSERT ON local_settings BEGIN SELECT RAISE(ABORT,'disk failure'); END").execute(&machine.pool).await.expect("fault");
    assert!(machine.register().await.is_err());
    sqlx::raw_sql("DROP TRIGGER fail")
        .execute(&machine.pool)
        .await
        .expect("restore");
    machine.register().await.expect("retry");
    assert!(machine.advertise(true).await.is_err());
    machine.close().await;
    task.abort();
}

#[tokio::test]
async fn agent_refreshes_router_address_and_retains_it_during_directory_outage() {
    let reply = Arc::new(Mutex::new("https://router.test".to_owned()));
    let shared = reply.clone();
    let app = Router::new()
        .route(
            "/api/v1/installations/heartbeat",
            post(|| async { Json(json!({"ok":true})) }),
        )
        .route(
            "/api/v1/installations/router-address",
            post(move || {
                let shared = shared.clone();
                async move { Json(json!({"router_url":shared.lock().expect("lock").clone()})) }
            }),
        );
    let (origin, task) = server(app).await;
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    let mut identity = registration();
    identity.role = InstallationRole::Agent;
    identity.router_url = None;
    machine
        .initialize(&identity, &origin, "http://127.0.0.1:8080")
        .await
        .expect("identity");
    machine
        .save_setting("registered", "true")
        .await
        .expect("registered");
    sqlx::raw_sql("UPDATE remote_access SET state='ready'")
        .execute(&machine.pool)
        .await
        .expect("ready remote");
    machine.advertise(true).await.expect("refresh");
    assert_eq!(
        machine.router_origin().await.expect("cached"),
        "https://router.test"
    );
    *reply.lock().expect("lock") = "invalid".into();
    machine
        .advertise(true)
        .await
        .expect("invalid reply ignored");
    assert_eq!(
        machine.router_origin().await.expect("cached"),
        "https://router.test"
    );
    *reply.lock().expect("lock") = "https://router-new.test".into();
    sqlx::raw_sql("CREATE TRIGGER fail BEFORE INSERT ON local_settings BEGIN SELECT RAISE(ABORT,'write failure'); END").execute(&machine.pool).await.expect("fault");
    assert!(machine.advertise(true).await.is_err());
    task.abort();
    assert!(machine.advertise(true).await.is_err());
    assert_eq!(
        machine.router_origin().await.expect("cached"),
        "https://router.test"
    );
    sqlx::raw_sql("DROP TABLE remote_access")
        .execute(&machine.pool)
        .await
        .expect("fault");
    assert!(machine.advertise(true).await.is_err());
    sqlx::raw_sql("DROP TABLE installation")
        .execute(&machine.pool)
        .await
        .expect("fault");
    assert!(machine.router_origin().await.is_err());
    machine.close().await;
}

#[tokio::test]
async fn co_located_router_reports_missing_local_address_storage() {
    let dir = tempfile::tempdir().expect("directory");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    machine
        .save_setting("router_origin", "https://router.test")
        .await
        .expect("registered");
    sqlx::raw_sql("DROP TABLE remote_access")
        .execute(&machine.pool)
        .await
        .expect("fault");
    assert!(machine.router_origin().await.is_err());
    machine.close().await;
}
