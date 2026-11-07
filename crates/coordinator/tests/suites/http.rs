use crate::tests::{common::Fixture, data};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
};
use orchestrator_coordinator::{Error, web};
use tower::ServiceExt;

async fn call(
    router: axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", token);
    }
    router
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("infallible router")
}

#[tokio::test]
async fn http_authentication_limits_and_group_scoped_context() {
    let f = Fixture::enrolled().await;
    let router = web::router(f.store.clone());
    let auth = format!("Bearer {}", data::token().expose());
    let report = serde_json::to_vec(&data::report()).expect("report");
    for token in [
        None,
        Some("Basic abc"),
        Some("Bearer malformed"),
        Some("Bearer bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    ] {
        for (method, path, body) in [
            ("POST", "/v1/sync", report.clone()),
            ("GET", "/v1/nodes", vec![]),
            ("GET", "/v1/events", vec![]),
        ] {
            assert_eq!(
                call(router.clone(), method, path, token, body)
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
    }
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/v1/sync",
            Some(&auth),
            report.clone()
        )
        .await
        .status(),
        StatusCode::OK
    );
    for path in ["/v1/nodes", "/v1/events?after=0"] {
        let response = call(router.clone(), "GET", path, Some(&auth), vec![]).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body");
        assert!(!String::from_utf8_lossy(&bytes).contains(data::token().expose()));
    }
    f.close().await;
}

#[tokio::test]
async fn malformed_requests_conflicts_and_storage_errors_are_explicit() {
    let f = Fixture::enrolled().await;
    f.store
        .sync(&data::token(), &data::report())
        .await
        .expect("initial event");
    let router = web::router(f.store.clone());
    let auth = format!("Bearer {}", data::token().expose());
    let report = serde_json::to_vec(&data::report()).expect("report");
    assert_eq!(
        call(
            router.clone(),
            "GET",
            "/v1/events?after=-1",
            Some(&auth),
            vec![]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            router.clone(),
            "GET",
            "/v1/events?unknown=1",
            Some(&auth),
            vec![]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut invalid = data::report();
    invalid.profile.capacity = 0;
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/v1/sync",
            Some(&auth),
            serde_json::to_vec(&invalid).expect("body")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut conflict = data::report();
    conflict.events[0].outcome = orchestrator_protocol::sync::CommandOutcome::Rejected {
        reason: "changed".into(),
    };
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/v1/sync",
            Some(&auth),
            serde_json::to_vec(&conflict).expect("body")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/v1/sync",
            Some(&auth),
            vec![b' '; 4 * 1024 * 1024 + 1]
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let repeated = Request::builder()
        .uri("/v1/nodes")
        .header("authorization", &auth)
        .header("authorization", &auth)
        .body(Body::empty())
        .expect("request");
    assert_eq!(
        router
            .clone()
            .oneshot(repeated)
            .await
            .expect("response")
            .status(),
        StatusCode::UNAUTHORIZED
    );
    f.store.close().await;
    let response = call(router, "POST", "/v1/sync", Some(&auth), report).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = to_bytes(response.into_body(), 1000).await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON error");
    assert_eq!(value["error"], "coordination storage unavailable");
    f.close().await;
}

#[test]
fn migration_errors_have_no_public_internal_details() {
    let response =
        Error::Migration(sqlx::migrate::MigrateError::VersionMissing(999)).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
