use crate::tests::{
    common::Server,
    data,
    routing::{id, request},
};
use axum::{Json, Router, http::StatusCode, routing::post};
use orchestrator_coordinator::{
    Error,
    selector::{LlmSelector, Selector, Unconfigured},
};
use orchestrator_protocol::routing::{Candidate, RoutingContext};
use serde_json::{Value, json};

fn context() -> RoutingContext {
    RoutingContext {
        active_work: vec![],
        request: request(100),
        candidates: vec![Candidate {
            node_id: id(2),
            profile: data::report().profile,
            available_slots: 1,
        }],
        recent_activity: vec![],
    }
}

pub fn response(text: &str) -> Value {
    json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":text}]}]})
}

#[tokio::test]
async fn structured_model_response_is_bounded_and_only_selects_an_eligible_identity() {
    let server = Server::start(Router::new().route(
        "/",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["instructions"], "custom router instructions");
            assert_eq!(body["store"], false);
            assert!(body.get("tools").is_none());
            assert_eq!(body["text"]["format"]["strict"], true);
            let choice =
                body["text"]["format"]["schema"]["properties"]["node_id"]["enum"][0].clone();
            Json(response(&json!({"node_id":choice}).to_string()))
        }),
    ))
    .await;
    let temp = tempfile::tempdir().expect("prompts");
    std::fs::create_dir(temp.path().join("router")).expect("directory");
    std::fs::write(
        temp.path().join("router/selection.md"),
        "custom router instructions",
    )
    .expect("prompt");
    let selector = LlmSelector::new(&server.origin, "test-key", "test-model")
        .expect("selector")
        .with_prompt(
            orchestrator_prompts::Prompt::load(temp.path(), orchestrator_prompts::Kind::Router)
                .expect("load"),
        );
    assert_eq!(selector.choose(&context()).await.expect("choose"), id(2));
    assert!(matches!(
        selector
            .choose(&RoutingContext {
                candidates: vec![],
                ..context()
            })
            .await,
        Err(Error::NoCapacity)
    ));
    assert!(matches!(
        Unconfigured.choose(&context()).await,
        Err(Error::Model)
    ));
    server.close().await;
    assert!(matches!(
        selector.choose(&context()).await,
        Err(Error::Model)
    ));
    let invalid = [
        json!({"status":"completed"}),
        response(&json!({"node_id":id::<orchestrator_protocol::ids::NodeId>(99)}).to_string()),
        response("bad"),
        response(
            &json!({"node_id":id::<orchestrator_protocol::ids::NodeId>(2),"extra":true})
                .to_string(),
        ),
        json!({"status":"incomplete"}),
        json!({"status":"completed","output":[{"type":"reasoning"},{"type":"message","content":[{"type":"refusal"},{"type":"output_text","text":123}]}]}),
        json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"{}"},{"type":"output_text","text":"{}"}]}]}),
    ];
    for value in invalid {
        let server = Server::start(Router::new().route(
            "/",
            post(move || {
                let value = value.clone();
                async move { Json(value) }
            }),
        ))
        .await;
        assert!(matches!(
            LlmSelector::new(&server.origin, "key", "model")
                .expect("config")
                .choose(&context())
                .await,
            Err(Error::Model)
        ));
        server.close().await;
    }
    for (status, body) in [
        (StatusCode::BAD_GATEWAY, "unavailable".into()),
        (StatusCode::FOUND, "redirect".into()),
        (StatusCode::OK, "not json".into()),
        (StatusCode::OK, "x".repeat(262_145)),
    ] {
        let server = Server::start(Router::new().route(
            "/",
            post(move || {
                let body = body.clone();
                async move { (status, body) }
            }),
        ))
        .await;
        assert!(matches!(
            LlmSelector::new(&server.origin, "key", "model")
                .expect("config")
                .choose(&context())
                .await,
            Err(Error::Model)
        ));
        server.close().await;
    }
}
