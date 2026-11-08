#![allow(clippy::expect_used)]
use super::*;
#[test]
fn release_selection_requires_newer_complete_supported_versions() {
    for value in ["0.2.0", "vbad", "v0.2.0+local", "v0.2.0-beta.1"] {
        assert!(version(value).is_err());
    }
    assert!(version("v0.2.0").is_ok());
    for (os, arch) in [
        ("macos", "aarch64"),
        ("macos", "x86_64"),
        ("linux", "aarch64"),
        ("linux", "x86_64"),
    ] {
        assert!(target(os, arch).is_ok());
    }
    assert!(target("windows", "x86_64").is_err());
    let release = |tag: &str, draft, complete| Release {
        tag_name: tag.into(),
        draft,
        assets: if complete {
            vec![
                Asset {
                    name: format!("cuttlefish-{tag}-target.tar.gz"),
                },
                Asset {
                    name: "SHA256SUMS".into(),
                },
            ]
        } else {
            vec![]
        },
    };
    let releases = vec![
        release("v0.2.0-alpha.6", false, true),
        release("v0.2.0-alpha.8", false, true),
        release("v0.2.0-alpha.7", false, true),
        release("v0.2.0-alpha.99", true, true),
        release("v0.2.0-alpha.100", false, false),
        release("bad", false, true),
    ];
    assert_eq!(
        latest(releases, "v0.2.0-alpha.6", "target")
            .expect("latest")
            .as_deref(),
        Some("v0.2.0-alpha.8")
    );
    assert!(latest(vec![], "bad", "target").is_err());
    assert!(latest(vec![], "v0.2.0", "target").expect("empty").is_none());
}
#[tokio::test]
async fn http_catalog_rejects_errors_redirects_malformed_and_oversized_bodies() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (response, expected) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]".into(),
            true,
        ),
        (
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n".into(),
            false,
        ),
        (
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1\r\nContent-Length: 0\r\n\r\n"
                .into(),
            false,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad".into(),
            false,
        ),
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 2000001\r\n\r\n{}",
                "x".repeat(2_000_001)
            ),
            false,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n[".into(),
            false,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(response.as_bytes()).await;
        });
        assert_eq!(fetch(&url).await.is_ok(), expected);
        task.await.expect("server");
    }
    assert!(fetch("bad:").await.is_err());
}

#[tokio::test]
async fn invalid_transport_configuration_is_reported() {
    assert!(
        fetch_with(
            "http://127.0.0.1",
            reqwest::Client::builder().user_agent("invalid\nheader")
        )
        .await
        .is_err()
    );
}
