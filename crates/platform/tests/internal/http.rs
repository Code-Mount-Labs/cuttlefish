use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body")
        .to_vec();
    (status, headers, bytes)
}
async fn json_call(
    app: &axum::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
    expected: StatusCode,
) -> Value {
    let (status, _, bytes) = call(app, method, path, headers, body).await;
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).expect("JSON")
}
#[tokio::test]
async fn browser_and_mobile_sessions_enforce_origin_and_account_scope() {
    let f = Fixture::new().await;
    let app = web::router(f.platform.clone());
    for path in ["/", "/app.js", "/app.css", "/logo.png"] {
        let (status, headers, bytes) = call(&app, "GET", path, &[], Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!bytes.is_empty());
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["cache-control"], "no-store");
    }
    json_call(&app, "GET", "/healthz", &[], Value::Null, StatusCode::OK).await;
    let status = json_call(
        &app,
        "GET",
        "/api/v1/status",
        &[],
        Value::Null,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["service"], "cuttlefish-platform");
    let input = json!({"username":"web.user","password":"secret-test-password"});
    json_call(
        &app,
        "POST",
        "/api/v1/auth/register",
        &[("origin", "https://attacker.test")],
        input.clone(),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let (status, headers, body) =
        call(&app, "POST", "/api/v1/auth/register", &[], input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let session: Value = serde_json::from_slice(&body).expect("JSON");
    let bearer = format!("Bearer {}", session["token"].as_str().expect("token"));
    let cookie = headers["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("pair");
    assert!(
        headers["set-cookie"]
            .to_str()
            .expect("cookie")
            .contains("HttpOnly; SameSite=Strict")
    );
    json_call(
        &app,
        "GET",
        "/api/v1/auth/session",
        &[("cookie", cookie)],
        Value::Null,
        StatusCode::OK,
    )
    .await;
    check_browser_session(&app, &bearer, cookie, input).await;
    f.close().await;
}

async fn check_browser_session(app: &axum::Router, bearer: &str, cookie: &str, input: Value) {
    for headers in [
        vec![],
        vec![("authorization", "Basic abcd")],
        vec![("authorization", "Bearer wrong")],
        vec![("authorization", bearer), ("cookie", cookie)],
        vec![("cookie", cookie), ("cookie", cookie)],
    ] {
        json_call(
            app,
            "GET",
            "/api/v1/auth/session",
            &headers,
            Value::Null,
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    json_call(
        app,
        "POST",
        "/api/v1/groups",
        &[("cookie", cookie)],
        json!({"name":"Web"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    let auth = [("cookie", cookie), ("origin", "http://127.0.0.1:8080")];
    let group = json_call(
        app,
        "POST",
        "/api/v1/groups",
        &auth,
        json!({"name":"Web"}),
        StatusCode::OK,
    )
    .await;
    let group = group["group_id"].as_str().expect("group");
    let groups = json_call(
        app,
        "GET",
        "/api/v1/groups",
        &[("authorization", bearer)],
        Value::Null,
        StatusCode::OK,
    )
    .await;
    assert_eq!(groups.as_array().expect("groups").len(), 1);
    json_call(
        app,
        "GET",
        &format!("/api/v1/groups/{group}/installations"),
        &auth,
        Value::Null,
        StatusCode::OK,
    )
    .await;
    json_call(
        app,
        "POST",
        &format!("/api/v1/groups/{group}/invitations"),
        &auth,
        json!({}),
        StatusCode::OK,
    )
    .await;
    json_call(
        app,
        "POST",
        "/api/v1/auth/logout",
        &auth,
        json!({}),
        StatusCode::OK,
    )
    .await;
    json_call(
        app,
        "GET",
        "/api/v1/auth/session",
        &[("cookie", cookie)],
        Value::Null,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    json_call(
        app,
        "POST",
        "/api/v1/auth/login",
        &[],
        input,
        StatusCode::OK,
    )
    .await;
}
#[tokio::test]
async fn installation_json_api_supports_exact_retries_and_authenticated_heartbeats() {
    let f = Fixture::new().await;
    let app = web::router(f.platform.clone());
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let input = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::RouterAgent,
        100,
    );
    let preflight = json!({"group_id":group,"invitation":input.invitation.expose()});
    json_call(
        &app,
        "POST",
        "/api/v1/installations/preflight",
        &[],
        preflight,
        StatusCode::OK,
    )
    .await;
    for _ in 0..2 {
        json_call(
            &app,
            "POST",
            "/api/v1/installations",
            &[],
            serde_json::to_value(&input).expect("registration"),
            StatusCode::OK,
        )
        .await;
    }
    let bearer = format!("Bearer {}", input.credential.expose());
    let auth = [("authorization", bearer.as_str())];
    let view = json_call(
        &app,
        "GET",
        "/api/v1/installations/self",
        &auth,
        Value::Null,
        StatusCode::OK,
    )
    .await;
    assert_eq!(view["online"], false);
    assert!(view.get("credential").is_none());
    json_call(
        &app,
        "POST",
        "/api/v1/installations/heartbeat",
        &auth,
        serde_json::to_value(heartbeat(input.role)).expect("heartbeat"),
        StatusCode::OK,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/router/authorize-agent",
        &auth,
        json!({"installation_id":input.installation_id,"credential":input.credential.expose()}),
        StatusCode::OK,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/router/authorize-agent",
        &auth,
        json!({"installation_id":input.installation_id,"credential":"bad"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/installations/preflight",
        &[],
        json!({"group_id":group,"invitation":"bad"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/auth/register",
        &[],
        json!({"username":"x","password":"short"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/auth/register",
        &[],
        json!({"username":"owner","password":"test-password-123"}),
        StatusCode::CONFLICT,
    )
    .await;
    check_http_limits(&f, &app).await;
    f.close().await;
}

async fn check_http_limits(f: &Fixture, app: &axum::Router) {
    let permits = f
        .platform
        .password_slots
        .acquire_many(2)
        .await
        .expect("permits");
    json_call(
        app,
        "POST",
        "/api/v1/auth/login",
        &[],
        json!({"username":"owner","password":"test-password-123"}),
        StatusCode::TOO_MANY_REQUESTS,
    )
    .await;
    drop(permits);
    f.platform.close().await;
    json_call(
        app,
        "GET",
        "/healthz",
        &[],
        Value::Null,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
}

#[tokio::test]
async fn every_authenticated_route_rejects_missing_and_unknown_credentials() {
    let f = Fixture::new().await;
    let app = web::router(f.platform.clone());
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let input = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::RouterAgent,
        800,
    );
    let routes = [
        ("GET", "/api/v1/groups".into(), Value::Null),
        ("POST", "/api/v1/groups".into(), json!({"name":"name"})),
        (
            "GET",
            format!("/api/v1/groups/{group}/installations"),
            Value::Null,
        ),
        (
            "POST",
            format!("/api/v1/groups/{group}/invitations"),
            json!({}),
        ),
        ("GET", "/api/v1/installations/self".into(), Value::Null),
        (
            "POST",
            "/api/v1/installations/heartbeat".into(),
            serde_json::to_value(heartbeat(input.role)).expect("heartbeat"),
        ),
        (
            "POST",
            "/api/v1/router/authorize-agent".into(),
            json!({"installation_id":input.installation_id,"credential":input.credential.expose()}),
        ),
    ];
    let unknown = format!("Bearer {}", "f".repeat(64));
    for (method, path, body) in routes {
        for headers in [
            vec![("origin", "http://127.0.0.1:8080")],
            vec![("authorization", unknown.as_str())],
        ] {
            json_call(
                &app,
                method,
                &path,
                &headers,
                body.clone(),
                StatusCode::UNAUTHORIZED,
            )
            .await;
        }
    }
    json_call(
        &app,
        "POST",
        "/api/v1/auth/logout",
        &[],
        json!({}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/auth/login",
        &[("origin", "https://attacker.test")],
        json!({"username":"owner","password":"test-password-123"}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    json_call(
        &app,
        "POST",
        "/api/v1/installations/preflight",
        &[],
        json!({"group_id":group,"invitation":"f".repeat(64)}),
        StatusCode::UNAUTHORIZED,
    )
    .await;
    check_invalid_enrollment(&f, &app, &input, &unknown).await;
    f.close().await;
}

async fn check_invalid_enrollment(
    f: &Fixture,
    app: &axum::Router,
    input: &Registration,
    unknown: &str,
) {
    let mut rejected = serde_json::to_value(input).expect("registration");
    rejected["router_url"] = Value::Null;
    json_call(
        app,
        "POST",
        "/api/v1/installations",
        &[],
        rejected,
        StatusCode::BAD_REQUEST,
    )
    .await;
    f.platform.close().await;
    json_call(
        app,
        "POST",
        "/api/v1/auth/logout",
        &[("authorization", unknown)],
        json!({}),
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
}
#[tokio::test]
async fn cookies_reject_invalid_bytes_and_https_sessions_are_secure() {
    let mut f = Fixture::new().await;
    f.platform.origin = "https://platform.test".into();
    let app = web::router(f.platform.clone());
    let (status, headers, _) = call(
        &app,
        "POST",
        "/api/v1/auth/register",
        &[],
        json!({"username":"owner","password":"test-password-123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["set-cookie"]
            .to_str()
            .expect("cookie")
            .contains("; Secure")
    );
    json_call(
        &app,
        "GET",
        "/api/v1/auth/session",
        &[("cookie", "unrelated=value")],
        Value::Null,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    for name in ["cookie", "authorization"] {
        let request = Request::builder()
            .uri("/api/v1/auth/session")
            .header(
                name,
                axum::http::HeaderValue::from_bytes(&[255]).expect("header"),
            )
            .body(Body::empty())
            .expect("request");
        assert_eq!(
            app.clone()
                .oneshot(request)
                .await
                .expect("response")
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    f.close().await;
}
