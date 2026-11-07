#![allow(clippy::expect_used)]
use super::*;
use crate::tests::{
    Fixture,
    relay::{configured, installation, mutation},
};
use axum::http::HeaderValue;

fn reject_configuration(platform: &crate::Platform) {
    for value in [
        "bad",
        "https://127.0.0.1",
        "http://example.com",
        "http://127.0.0.1/path",
        "http://user@127.0.0.1",
        "http://127.0.0.1?x",
        "http://127.0.0.1#x",
    ] {
        assert!(router(platform.clone(), value).is_err());
    }
    assert!(
        with_builder(
            platform.clone(),
            "http://127.0.0.1",
            reqwest::Client::builder().user_agent("bad\nheader")
        )
        .is_err()
    );
}
async fn reject_duplicate_headers(active: &Gateway, id: Uuid) {
    let request = || {
        Request::builder()
            .uri("/api/local/status")
            .body(Body::empty())
            .expect("request")
    };
    for name in [header::AUTHORIZATION, header::CONTENT_TYPE, header::ORIGIN] {
        let mut req = request();
        req.headers_mut()
            .append(&name, HeaderValue::from_static("https://relay.test"));
        req.headers_mut()
            .append(name, HeaderValue::from_static("https://relay.test"));
        assert!(matches!(
            forward(
                State(active.clone()),
                Path((id, "api/local/status".into())),
                req
            )
            .await,
            Err(Error::Unauthorized)
        ));
    }
}
#[tokio::test]
async fn gateway_configuration_and_unavailable_storage_fail_closed() {
    let f = Fixture::new().await;
    reject_configuration(&f.platform);
    let gateway = Gateway {
        platform: f.platform.clone(),
        client: reqwest::Client::new(),
        upstream: "http://127.0.0.1:1".into(),
        slots: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let own = installation(&f, 710).await;
    let platform = configured(f.platform.clone());
    platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    let id: Uuid = own.installation_id.into();
    let mut active = gateway.clone();
    active.platform = platform;
    let request = || {
        Request::builder()
            .uri("/api/local/status")
            .body(Body::empty())
            .expect("request")
    };
    assert!(matches!(
        forward(
            State(active.clone()),
            Path((id, "api/local/status".into())),
            request()
        )
        .await,
        Err(Error::Limited)
    ));
    active.slots.add_permits(1);
    let missing_keys = gateway.clone();
    missing_keys.slots.add_permits(1);
    assert!(
        forward(
            State(missing_keys),
            Path((id, "api/local/status".into())),
            request()
        )
        .await
        .is_err()
    );
    let mut bad_cookie = request();
    bad_cookie.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_static("cuttlefish_local_session=bad"),
    );
    assert!(
        forward(
            State(active.clone()),
            Path((id, "api/local/status".into())),
            bad_cookie
        )
        .await
        .is_err()
    );

    reject_duplicate_headers(&active, id).await;
    assert!(matches!(
        forward(
            State(active.clone()),
            Path((id, "api/local/status".into())),
            request()
        )
        .await,
        Err(Error::Transport("relay_connection_unavailable"))
    ));
    let large = Request::builder()
        .uri("/api/local/status")
        .body(Body::from(vec![0; 1_048_577]))
        .expect("request");
    assert!(matches!(
        forward(State(active), Path((id, "api/local/status".into())), large).await,
        Err(Error::Invalid("relay_request_too_large"))
    ));
    f.platform.close().await;
    assert!(health(State(gateway.clone())).await.is_err());
    assert!(
        index(
            State(gateway),
            Path(id),
            OriginalUri("/i/x/".parse().expect("uri"))
        )
        .await
        .is_err()
    );
    f.close().await;
}
#[test]
fn cookies_are_single_scoped_and_never_inherit_remote_attributes() {
    let id = Uuid::new_v4();
    for value in [
        "other=value",
        "cuttlefish_local_session=bad",
        "cuttlefish_local_session=; Max-Age=bad",
        "cuttlefish_local_session=; Max-Age=28801",
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(header::SET_COOKIE, value.parse().expect("value"));
        assert!(response_cookie(&headers, id).is_err());
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_bytes(&[255]).expect("opaque"),
    );
    assert!(response_cookie(&headers, id).is_err());
    headers.append(header::SET_COOKIE, HeaderValue::from_static("other=value"));
    assert!(response_cookie(&headers, id).is_err());
    headers.clear();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_static("cuttlefish_local_session=; Max-Age=0"),
    );
    assert!(
        response_cookie(&headers, id)
            .expect("logout")
            .expect("cookie")
            .contains("Max-Age=0")
    );
    for value in [
        HeaderValue::from_bytes(&[255]).expect("opaque"),
        HeaderValue::from_static("cuttlefish_local_session=bad"),
        format!(
            "cuttlefish_local_session={}; cuttlefish_local_session={}",
            "a".repeat(64),
            "b".repeat(64)
        )
        .parse()
        .expect("value"),
    ] {
        headers.clear();
        headers.insert(header::COOKIE, value);
        assert!(session_cookie(&headers).is_err());
    }
}
#[tokio::test]
async fn invalid_response_cookie_and_truncated_body_never_reach_browser() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let id = Uuid::new_v4();
    let invalid = reqwest::Response::from(
        axum::http::Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::SET_COOKIE, "untrusted=cookie")
            .body("{}".to_owned())
            .expect("response"),
    );
    assert!(json_response(invalid, id).await.is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = [0; 4096];
        assert!(stream.read(&mut buf).await.expect("read") > 0);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").await.expect("write");
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    let response = reqwest::get(format!("http://{address}"))
        .await
        .expect("headers");
    assert!(matches!(
        json_response(response, id).await,
        Err(Error::Transport("relay_response_interrupted"))
    ));
    task.await.expect("task");
    let request = Request::builder()
        .method("DELETE")
        .uri("/api/local/status")
        .body(Body::empty())
        .expect("request");
    assert!(validate_request(&request, "api/local/status", "https://relay.test").is_err());
}
