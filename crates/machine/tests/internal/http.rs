use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
pub(crate) async fn request(
    app: &Router,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut request = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let body = if let Some(body) = body {
        request = request
            .method("POST")
            .header("content-type", "application/json");
        Body::from(serde_json::to_vec(&body).expect("JSON"))
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers().contains_key("content-security-policy"));
    let status = response.status();
    let cookie = response
        .headers()
        .get("set-cookie")
        .map(|v| v.to_str().expect("cookie").to_owned());
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
        cookie,
    )
}
pub(crate) async fn sign_in(app: &Router, machine: &Machine) -> String {
    let bootstrap = machine.bootstrap_token().expect("token");
    let (status,_,cookie)=request(app,"/api/local/setup",Some(json!({"token":bootstrap.expose(),"username":"operator","password":"a sufficiently long secret"})),None,Some("http://127.0.0.1:8080")).await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.expect("cookie");
    assert!(cookie.contains("HttpOnly; SameSite=Strict"));
    cookie.split(';').next().expect("pair").to_owned()
}
#[tokio::test]
async fn dashboard_supports_deferred_setup_authenticated_configuration_and_logout() {
    let dir = tempfile::tempdir().expect("dir");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(dir.path(), backend.clone())
        .await
        .expect("machine");
    initialize(&machine).await;
    let app = crate::web::router(machine.clone());
    public_assets(&app).await;
    assert_eq!(
        request(&app, "/api/local/status", None, None, None).await.1,
        json!({"initialized":false,"version":env!("CARGO_PKG_VERSION")})
    );
    assert_eq!(
        request(&app, "/api/local/config", None, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&app, &machine).await;
    let (status, config, _) = request(&app, "/api/local/config", None, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["database"]["state"], "unconfigured");
    assert!(
        !config
            .to_string()
            .contains(registration().credential.expose())
    );
    assert_eq!(
        request(&app, "/v1/nodes", None, None, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for origin in [None, Some("https://evil.example")] {
        assert_eq!(
            request(
                &app,
                "/api/local/database",
                Some(json!({"mode":"later"})),
                Some(&cookie),
                origin
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let origin = Some("http://127.0.0.1:8080");
    assert_eq!(
        request(
            &app,
            "/api/local/database",
            Some(json!({"mode":"later"})),
            Some(&cookie),
            origin
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    exercise_database_recovery(&app, &machine, &backend, &cookie).await;
    assert_eq!(request(&app,"/api/local/model",Some(json!({"endpoint":"https://provider.example/v1/responses","model":"test-model","api_key":"private-api-key"})),Some(&cookie),origin).await.0,StatusCode::OK);
    let config = request(&app, "/api/local/config", None, Some(&cookie), None)
        .await
        .1;
    assert_eq!(config["model"]["model"], "test-model");
    assert!(!config.to_string().contains("private"));
    assert_eq!(
        request(
            &app,
            "/api/local/logout",
            Some(json!({})),
            Some(&cookie),
            origin
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/local/config", None, Some(&cookie), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/api/local/login",
            Some(json!({"username":"operator","password":"a sufficiently long secret"})),
            None,
            origin
        )
        .await
        .0,
        StatusCode::OK
    );
    machine.close().await;
}
#[tokio::test]
async fn agent_dashboard_cannot_configure_a_database_or_routing_model() {
    let dir = tempfile::tempdir().expect("dir");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    let mut registration = registration();
    registration.role = InstallationRole::Agent;
    registration.router_url = None;
    machine
        .initialize(
            &registration,
            "http://127.0.0.1:8081",
            "http://127.0.0.1:8080",
        )
        .await
        .expect("identity");
    let app = crate::web::router(machine.clone());
    let cookie = sign_in(&app, &machine).await;
    for (path, body) in [
        ("database", json!({"mode":"later"})),
        ("database/retry", json!({})),
        (
            "model",
            json!({"endpoint":"https://provider.example","model":"model","api_key":"key"}),
        ),
    ] {
        assert_eq!(
            request(
                &app,
                &format!("/api/local/{path}"),
                Some(body),
                Some(&cookie),
                Some("http://127.0.0.1:8080")
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(machine.execution_store().await.is_err());
    assert_eq!(
        request(
            &app,
            "/api/local/config",
            None,
            Some(&format!("{cookie};{cookie}")),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/api/node/enroll",
            Some(json!({"installation_id":registration.installation_id,"credential":"short"})),
            None,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    machine.close().await;
}

async fn exercise_database_recovery(
    app: &Router,
    machine: &Machine,
    backend: &Backend,
    cookie: &str,
) {
    let origin = Some("http://127.0.0.1:8080");
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            app,
            "/api/local/database",
            Some(
                json!({"mode":"existing","url":"postgresql://owner:private-password@localhost/db"})
            ),
            Some(cookie),
            origin
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert!(machine.recover().await.is_err());
    assert_eq!(
        machine
            .status()
            .await
            .expect("failed")
            .error_code
            .as_deref(),
        Some("simulated_failure")
    );
    backend.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        request(
            app,
            "/api/local/database/retry",
            Some(json!({})),
            Some(cookie),
            origin
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    machine
        .recover()
        .await
        .expect("worker finishes queued retry");
}

async fn public_assets(app: &Router) {
    for path in ["/", "/app.js", "/app.css"] {
        assert_eq!(request(app, path, None, None, None).await.0, StatusCode::OK);
    }
    assert_eq!(
        request(app, "/missing", None, None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn updates_require_admin_and_same_origin_and_report_unmanaged_layout() {
    let dir = tempfile::tempdir().expect("temp");
    let machine = Machine::open(dir.path(), Arc::new(Backend::default()))
        .await
        .expect("machine");
    initialize(&machine).await;
    let app = crate::web::router(machine.clone());
    assert_eq!(
        request(&app, "/api/local/updates", None, None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let cookie = sign_in(&app, &machine).await;
    let (status, value, _) = request(&app, "/api/local/updates", None, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["supported"], false);
    for origin in [None, Some("https://evil.example")] {
        assert_eq!(
            request(
                &app,
                "/api/local/updates",
                Some(json!({"action":"restart"})),
                Some(&cookie),
                origin
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, _, _) = request(
        &app,
        "/api/local/updates",
        Some(json!({"action":"check"})),
        Some(&cookie),
        Some("http://127.0.0.1:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    machine.close().await;
}
