#![allow(clippy::expect_used)]
use super::*;
#[tokio::test]
async fn disconnected_and_truncated_collaboration_responses_are_errors() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let token = "a".repeat(64).try_into().expect("token");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let origin = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("connection");
        assert!(stream.read(&mut vec![0; 65536]).await.expect("request") > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .expect("headers");
        tokio::time::sleep(Duration::from_millis(50)).await;
        stream.shutdown().await.expect("close");
    });
    let remote = Remote::new(&origin, &token).expect("remote");
    assert!(
        remote
            .exchange("/v1/goals", Method::GET, None)
            .await
            .is_err()
    );
    server.await.expect("server");
    assert!(
        remote
            .exchange("/v1/goals", Method::GET, None)
            .await
            .is_err()
    );
}
