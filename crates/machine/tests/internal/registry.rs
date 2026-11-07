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
