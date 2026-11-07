#![allow(clippy::expect_used)]
use super::*;
#[path = "../../../protocol/tests/fixtures/mod.rs"]
mod data;
fn context() -> PlanningContext {
    PlanningContext {
        installations: vec![],
        goal: orchestrator_domain::collaboration::create(data::goal(), data::id(2)),
        peers: vec![],
        catalog: vec![data::release(300)],
        native: Capability::native(),
        workspaces: vec![data::id(5)],
    }
}
#[test]
fn configuration_and_malformed_decisions_fail_closed() {
    for endpoint in [
        "bad",
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com?q=x",
        "https://example.com#x",
    ] {
        assert!(ModelPlanner::new(endpoint, "key", "model").is_err());
    }
    for (key, model) in [
        ("", "model"),
        ("key", ""),
        ("key\n", "model"),
        ("key", &"m".repeat(129)),
    ] {
        assert!(ModelPlanner::new("https://example.com", key, model).is_err());
    }
    assert!(
        ModelPlanner::with_client(
            "https://example.com",
            "key",
            "model",
            Client::builder().user_agent("\n")
        )
        .is_err()
    );
    let planner = ModelPlanner::new("https://example.com", "secret-key", "model")
        .expect("config")
        .with_prompt(orchestrator_prompts::Prompt::bundled(
            orchestrator_prompts::Kind::Planner,
        ));
    assert!(!format!("{planner:?}").contains("secret-key"));
    for value in [
        json!({}),
        json!({"status":"completed"}),
        json!({"status":"completed","output":[]}),
        response("not json"),
        json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"{}"},{"type":"output_text","text":"{}"}]}]}),
    ] {
        assert!(decode(&value).is_err());
    }
    assert!(
        decode(&response("{\"command\":null}"))
            .expect("wait")
            .is_none()
    );
    let action = data::change(
        &context().goal,
        orchestrator_protocol::collaboration::GoalAction::Pause {},
    );
    assert_eq!(
        decode(&response(&json!({"command":action}).to_string())).expect("decision"),
        Some(action)
    );
    let evaluation = data::change(
        &context().goal,
        orchestrator_protocol::collaboration::GoalAction::Evaluate {
            release_id: data::id(300),
            passed: true,
            evidence: "unverified".to_owned().try_into().expect("text"),
        },
    );
    assert!(decode(&response(&json!({"command":evaluation}).to_string())).is_err());
}
fn response(text: &str) -> Value {
    json!({"status":"completed","output":[{"type":"reasoning"},{"type":"message","content":[{"type":"refusal"},{"type":"output_text","text":text}]}]})
}
#[tokio::test]
async fn model_calls_are_bounded_and_decisions_are_typed() {
    use axum::{Json, Router, http::StatusCode, routing::post};
    let prompts = tempfile::tempdir().expect("prompts");
    std::fs::create_dir(prompts.path().join("super-agent")).expect("directory");
    std::fs::write(
        prompts.path().join("super-agent/planner.md"),
        "custom planner instructions",
    )
    .expect("prompt");
    let prompt =
        orchestrator_prompts::Prompt::load(prompts.path(), orchestrator_prompts::Kind::Planner)
            .expect("load");
    for (status, body, success) in [
        (
            StatusCode::OK,
            response("{\"command\":null}").to_string(),
            true,
        ),
        (StatusCode::BAD_GATEWAY, "error".into(), false),
        (StatusCode::OK, "bad json".into(), false),
        (StatusCode::OK, "x".repeat(262_145), false),
        (
            StatusCode::OK,
            json!({"status":"incomplete"}).to_string(),
            false,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let endpoint = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/",
                    post(move |Json(request): Json<Value>| {
                        let body = body.clone();
                        async move {
                            assert_eq!(request["instructions"], "custom planner instructions");
                            assert_eq!(request["store"], false);
                            assert!(request.get("tools").is_none());
                            (status, body)
                        }
                    }),
                ),
            )
            .await
            .expect("serve");
        });
        let planner = ModelPlanner::new(&endpoint, "key", "model")
            .expect("model")
            .with_prompt(prompt.clone());
        assert_eq!(planner.decide(&context()).await.is_ok(), success);
        let mut large = context();
        large.catalog = vec![data::release(300); 10000];
        assert!(planner.decide(&large).await.is_err());
        server.abort();
        assert!(planner.decide(&context()).await.is_err());
    }
}

#[tokio::test]
async fn a_truncated_response_is_not_accepted_as_a_decision() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stream.shutdown().await.expect("close");
    });
    assert!(
        ModelPlanner::new(&origin, "key", "model")
            .expect("config")
            .decide(&context())
            .await
            .is_err()
    );
    server.await.expect("server");
}
