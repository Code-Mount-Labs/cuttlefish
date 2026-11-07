use super::*;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use tower::ServiceExt;
async fn send(
    app: Router,
    path: &str,
    method: &str,
    headers: &[(&str, String)],
    body: Body,
) -> axum::response::Response {
    let mut request = Request::builder().uri(path).method(method);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    app.oneshot(request.body(body).expect("request"))
        .await
        .expect("response")
}
async fn reject_token_requests(app: &Router, id: InstallationId) {
    for credential in [
        "Bearer other".into(),
        "Basic ?".into(),
        format!("Basic {}", STANDARD.encode([255])),
        format!("Basic {}", STANDARD.encode("no-colon")),
        format!("Basic {}", STANDARD.encode("not-an-id:bad")),
        format!("Basic {}", STANDARD.encode(format!("{id}:bad"))),
    ] {
        let headers = [
            ("content-type", "application/x-www-form-urlencoded".into()),
            ("authorization", credential),
        ];
        assert_eq!(
            send(
                app.clone(),
                "/api/v1/relay/token",
                "POST",
                &headers,
                Body::from("grant_type=client_credentials")
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for body in ["grant_type=wrong", "grant_type=client_credentials"] {
        assert_eq!(
            send(
                app.clone(),
                "/api/v1/relay/token",
                "POST",
                &[("content-type", "application/x-www-form-urlencoded".into())],
                Body::from(body)
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
async fn reject_gateway_requests(app: &Router, base: &str) {
    for suffix in ["v1/html", "v1/malformed", "v1/redirect", "v1/large"] {
        assert_eq!(
            send(
                app.clone(),
                &format!("{base}/{suffix}"),
                "GET",
                &[],
                Body::empty()
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    for suffix in [
        "secret",
        "api/local/setup",
        "api/local/%63onfig",
        "api//local/config",
    ] {
        assert_eq!(
            send(
                app.clone(),
                &format!("{base}/{suffix}"),
                "POST",
                &[],
                Body::empty()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        send(
            app.clone(),
            &format!("{base}/api/local/config"),
            "GET",
            &[("origin", "https://evil.test".into())],
            Body::empty()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
}
async fn check_gateway_forwarding(app: &Router, base: &str, id: InstallationId, epoch: Uuid) {
    let response = send(
        app.clone(),
        &format!("{base}/api/local/config?q=1"),
        "GET",
        &[(
            "cookie",
            format!(
                "unrelated=private; cuttlefish_local_session={}",
                "d".repeat(64)
            ),
        )],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.expect("body"))
            .expect("json");
    assert_eq!(body["host"], format!("{id}.{epoch}.cuttlefish.invalid"));
    assert_eq!(body["path"], "/api/local/config?q=1");
    assert!(
        !body["cookie"]
            .as_str()
            .expect("cookie")
            .contains("unrelated")
    );
    let response = send(
        app.clone(),
        &format!("{base}/api/local/login"),
        "POST",
        &[
            ("origin", "https://relay.test".into()),
            ("content-type", "application/json".into()),
        ],
        Body::from("{}"),
    )
    .await;
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .expect("cookie");
    assert!(cookie.contains(&format!("Path={base}/")));
    assert!(cookie.contains("Secure"));
    assert!(!cookie.contains("evil.test"));
}
#[tokio::test]
async fn hosted_discovery_grant_and_token_apis_require_their_own_credentials() {
    let mut f = Fixture::new().await;
    let own = installation(&f, 640).await;
    let disabled = web::router(f.platform.clone());
    assert_eq!(
        send(disabled, "/api/v1/relay/keys", "GET", &[], Body::empty())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    f.platform = configured(f.platform.clone());
    let app = web::router(f.platform.clone());
    for path in ["/.well-known/openid-configuration", "/api/v1/relay/keys"] {
        assert_eq!(
            send(app.clone(), path, "GET", &[], Body::empty())
                .await
                .status(),
            StatusCode::OK
        );
    }
    let grant_headers = [
        ("content-type", "application/json".into()),
        (
            "authorization",
            format!("Bearer {}", own.credential.expose()),
        ),
    ];
    assert_eq!(
        send(
            app.clone(),
            "/api/v1/installations/relay",
            "POST",
            &grant_headers,
            Body::from(serde_json::to_vec(&mutation(1)).expect("json"))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let basic = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", own.installation_id, secret().expose()))
    );
    let headers = [
        ("content-type", "application/x-www-form-urlencoded".into()),
        ("authorization", basic),
    ];
    let response = send(
        app.clone(),
        "/api/v1/relay/token",
        "POST",
        &headers,
        Body::from("grant_type=client_credentials"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.expect("body"))
            .expect("json");
    assert_eq!(body["expires_in"], 300);
    assert!(
        f.platform
            .relay_operation(
                "Login",
                &login(
                    own.installation_id,
                    body["access_token"].as_str().expect("token")
                )
            )
            .await
            .is_ok()
    );
    reject_token_requests(&app, own.installation_id).await;
    let hook = crate::relay::web::plugin_router(f.platform.clone());
    for version in ["wrong", "0.1.0"] {
        let response = send(
            hook.clone(),
            &format!("/frp?version={version}&op=Unknown"),
            "POST",
            &[("content-type", "application/json".into())],
            Body::from(r#"{"content":{}}"#),
        )
        .await;
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.expect("body"))
                .expect("json");
        assert_eq!(body["reject"], true);
    }
    f.close().await;
}
async fn backend(request: Request<Body>) -> axum::response::Response {
    match request.uri().path() {
        "/api/local/login"=>([(header::SET_COOKIE,format!("cuttlefish_local_session={}; Domain=evil.test; Path=/; Max-Age=28800","d".repeat(64)))],Json(json!({"ok":true}))).into_response(),
        "/v1/html"=>([(header::CONTENT_TYPE,"text/html")],"<script>bad()</script>").into_response(),
        "/v1/malformed"=>([(header::CONTENT_TYPE,"application/json")],"not json").into_response(),
        "/v1/redirect"=>(StatusCode::FOUND,[(header::LOCATION,"https://evil.test")],Json(json!({}))).into_response(),
        "/v1/large"=>([(header::CONTENT_TYPE,"application/json")]," ".repeat(1_048_577)).into_response(),
        _=>Json(json!({"host":request.headers().get(header::HOST).and_then(|h|h.to_str().ok()),"cookie":request.headers().get(header::COOKIE).and_then(|h|h.to_str().ok()),"path":request.uri().to_string()})).into_response(),
    }
}
#[tokio::test]
async fn gateway_isolates_epochs_serves_trusted_assets_and_rejects_untrusted_content() {
    let mut f = Fixture::new().await;
    let own = installation(&f, 650).await;
    f.platform = configured(f.platform.clone());
    let lease = f
        .platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let upstream = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(any(backend)))
            .await
            .expect("backend");
    });
    let app = crate::relay::gateway::router(f.platform.clone(), &upstream).expect("gateway");
    let base = format!("/i/{}", own.installation_id);
    assert_eq!(
        send(app.clone(), "/healthz", "GET", &[], Body::empty())
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(app.clone(), &base, "GET", &[], Body::empty())
            .await
            .status(),
        StatusCode::TEMPORARY_REDIRECT
    );
    for path in [
        format!("{base}/"),
        format!("{base}/app.js"),
        format!("{base}/app.css"),
    ] {
        assert_eq!(
            send(app.clone(), &path, "GET", &[], Body::empty())
                .await
                .status(),
            StatusCode::OK
        );
    }
    check_gateway_forwarding(&app, &base, own.installation_id, lease.epoch).await;
    reject_gateway_requests(&app, &base).await;
    f.platform
        .change_relay(
            &own.credential,
            &RelayMutation {
                revision: 2,
                credential: None,
            },
        )
        .await
        .expect("revoke");
    assert_eq!(
        send(
            app.clone(),
            &format!("{base}/api/local/config"),
            "GET",
            &[],
            Body::empty()
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    server.abort();
    f.close().await;
}

#[tokio::test]
async fn credential_failures_at_each_public_relay_boundary_are_rejected() {
    let f = Fixture::new().await;
    let own = installation(&f, 740).await;
    let disabled = web::router(f.platform.clone());
    assert_eq!(
        send(
            disabled,
            "/.well-known/openid-configuration",
            "GET",
            &[],
            Body::empty()
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let app = web::router(configured(f.platform.clone()));
    for headers in [
        vec![("content-type", "application/json".into())],
        vec![
            ("content-type", "application/json".into()),
            (
                "authorization",
                format!("Bearer {}", own.credential.expose()),
            ),
        ],
    ] {
        assert!(
            !send(
                app.clone(),
                "/api/v1/installations/relay",
                "POST",
                &headers,
                Body::from(r#"{"revision":0,"credential":null}"#)
            )
            .await
            .status()
            .is_success()
        );
    }
    let headers = [
        ("content-type", "application/x-www-form-urlencoded".into()),
        (
            "authorization",
            format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", own.installation_id, "b".repeat(64)))
            ),
        ),
    ];
    assert_eq!(
        send(
            app,
            "/api/v1/relay/token",
            "POST",
            &headers,
            Body::from("grant_type=client_credentials")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    f.close().await;
}
