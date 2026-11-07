#![allow(clippy::expect_used)]
use super::*;
#[test]
fn client_configuration_failure_is_reported() {
    let builder = reqwest::Client::builder().user_agent("invalid\nheader");
    assert!(matches!(
        Client::with_builder("https://example.com", builder),
        Err(Error::Dependency("network_client_unavailable"))
    ));
}
#[tokio::test]
async fn interrupted_body_is_never_decoded_as_a_complete_reply() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let client = Client::new(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("client");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut bytes = [0; 4096];
        assert!(socket.read(&mut bytes).await.expect("request") > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
            .await
            .expect("partial reply");
        tokio::time::sleep(Duration::from_millis(20)).await;
    });
    assert!(matches!(
        client.post::<_, serde_json::Value>("/", None, &()).await,
        Err(Error::Dependency("response_interrupted"))
    ));
    server.await.expect("server");
}
