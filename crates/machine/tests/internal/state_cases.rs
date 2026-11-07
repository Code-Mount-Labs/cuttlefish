use super::*;
use crate::{
    auth::Credentials,
    settings::{ModelSettings, validate_origin},
};
#[tokio::test]
async fn rejected_selections_do_not_touch_the_backend_or_replace_ready_data() {
    let dir = tempfile::tempdir().expect("dir");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(dir.path(), backend.clone())
        .await
        .expect("machine");
    assert!(machine.identity().await.is_err());
    initialize(&machine).await;
    let mut different = registration();
    different.name = "different".to_owned().try_into().expect("name");
    assert!(
        machine
            .initialize(&different, "http://127.0.0.1:8081", "http://127.0.0.1:8080")
            .await
            .is_err()
    );
    assert!(machine.recover().await.is_err());
    for url in [
        "invalid",
        "https://host/db",
        "postgresql:///",
        "postgresql://host/",
        "postgresql://host/db#fragment",
    ] {
        assert!(
            machine
                .configure(DatabaseChoice::Existing { url: url.into() })
                .await
                .is_err()
        );
    }
    assert!(
        machine
            .configure(DatabaseChoice::Managed { port: 0 })
            .await
            .is_err()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    assert!(
        machine
            .configure(DatabaseChoice::Managed {
                port: listener.local_addr().expect("address").port()
            })
            .await
            .is_err()
    );
    assert!(backend.specs.lock().expect("specs").is_empty());
    backend.fail.store(true, Ordering::SeqCst);
    assert!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://localhost/wrong".into()
            })
            .await
            .is_err()
    );
    backend.fail.store(false, Ordering::SeqCst);
    machine
        .configure(DatabaseChoice::Existing {
            url: "postgresql://localhost/correct".into(),
        })
        .await
        .expect("corrected URL");
    assert_eq!(machine.status().await.expect("status").revision, 2);
    assert!(
        machine
            .configure(DatabaseChoice::Existing {
                url: "postgresql://localhost/different".into()
            })
            .await
            .is_err()
    );
    assert!(
        machine
            .configure_model(&ModelSettings {
                endpoint: "http://public.example".into(),
                model: "model".into(),
                api_key: "key".into()
            })
            .await
            .is_err()
    );
    machine.close().await;
}
fn credentials(username: &str, password: &str) -> Credentials {
    Credentials {
        username: username.into(),
        password: password.into(),
    }
}
#[tokio::test]
async fn authentication_rejects_invalid_expired_and_rate_limited_requests_durably() {
    let dir = tempfile::tempdir().expect("dir");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(dir.path(), backend.clone())
        .await
        .expect("machine");
    initialize(&machine).await;
    let token = machine.bootstrap_token().expect("bootstrap");
    assert!(
        machine
            .setup_admin(
                &registration().credential,
                credentials("operator", "long-enough-password")
            )
            .await
            .is_err()
    );
    for (username, password) in [
        ("A", "short"),
        ("UPPERCASE", "long-enough-password"),
        ("operator", "short"),
    ] {
        assert!(
            machine
                .setup_admin(&token, credentials(username, password))
                .await
                .is_err()
        );
    }
    assert!(
        machine
            .login(credentials("operator", "long-enough-password"))
            .await
            .is_err()
    );
    let session = machine
        .setup_admin(&token, credentials("operator", "long-enough-password"))
        .await
        .expect("admin");
    let held = machine.password_slots.acquire_many(2).await.expect("held");
    assert!(matches!(
        machine
            .login(credentials("operator", "long-enough-password"))
            .await,
        Err(Error::Limited)
    ));
    assert!(matches!(
        machine
            .setup_admin(&token, credentials("operator", "long-enough-password"))
            .await,
        Err(Error::Limited)
    ));
    drop(held);
    for _ in 0..5 {
        assert!(
            machine
                .login(credentials("operator", "wrong-but-long-enough"))
                .await
                .is_err()
        );
    }
    machine.close().await;
    drop(machine);
    let machine = Machine::open(dir.path(), backend).await.expect("restart");
    assert!(
        machine
            .login(credentials("operator", "long-enough-password"))
            .await
            .is_err()
    );
    sqlx::query("UPDATE local_admin SET login_window=unixepoch()-61")
        .execute(&machine.pool)
        .await
        .expect("advance window");
    machine
        .login(credentials("operator", "long-enough-password"))
        .await
        .expect("window expired");
    sqlx::query("UPDATE local_sessions SET expires_at=unixepoch()-1")
        .execute(&machine.pool)
        .await
        .expect("expire");
    assert!(
        machine
            .authenticate(&session.try_into().expect("session"))
            .await
            .is_err()
    );
    sqlx::query("UPDATE local_admin SET password_hash='corrupted'")
        .execute(&machine.pool)
        .await
        .expect("corruption");
    assert!(
        machine
            .login(credentials("operator", "long-enough-password"))
            .await
            .is_err()
    );
    machine.close().await;
}
#[tokio::test]
async fn editable_prompts_survive_restart_and_ready_databases_are_reverified() {
    let dir = tempfile::tempdir().expect("dir");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(dir.path(), backend.clone())
        .await
        .expect("machine");
    initialize(&machine).await;
    let prompt = dir.path().join("prompts/router/selection.md");
    assert!(prompt.is_file());
    std::fs::write(&prompt, "Reviewed custom instructions").expect("edit");
    machine
        .configure(DatabaseChoice::Existing {
            url: "postgresql://localhost/db".into(),
        })
        .await
        .expect("configure");
    machine.close().await;
    drop(machine);
    let machine = Machine::open(dir.path(), backend).await.expect("restart");
    assert_eq!(
        machine.status().await.expect("status").state,
        "provisioning"
    );
    assert_eq!(
        std::fs::read_to_string(prompt).expect("prompt"),
        "Reviewed custom instructions"
    );
    machine.recover().await.expect("verified");
    machine.close().await;
}
#[test]
fn credentials_are_redacted_and_remote_origins_require_tls() {
    for origin in [
        "bad",
        "http://localhost",
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com/path",
        "https://example.com?secret",
        "https://example.com#fragment",
        "file:///tmp/",
    ] {
        assert!(validate_origin(origin).is_err(), "{origin}");
    }
    for origin in [
        "https://example.com",
        "http://127.0.0.1:1234",
        "http://[::1]:1234",
    ] {
        assert_eq!(validate_origin(origin).expect("safe"), origin);
    }
    let secret = "unique-private-secret";
    let values = [
        format!("{:?}", credentials("operator", secret)),
        format!("{:?}", DatabaseChoice::Existing { url: secret.into() }),
        format!("{:?}", DatabaseSpec::Existing { url: secret.into() }),
        format!(
            "{:?}",
            ModelSettings {
                endpoint: secret.into(),
                model: secret.into(),
                api_key: secret.into()
            }
        ),
        format!(
            "{:?}",
            crate::registry::Enrollment {
                installation_id: registration().installation_id,
                credential: secret.into()
            }
        ),
    ];
    for value in values {
        assert!(!value.contains(secret));
    }
}

#[tokio::test]
async fn routing_uses_persisted_model_and_editable_prompts_and_rejects_corrupt_settings() {
    use axum::{Json, Router, routing::post};
    use orchestrator_coordinator::selector::Selector;
    use orchestrator_protocol::{
        capabilities::{Capability, Platform},
        routing::{Candidate, RouteAction, RouteRequest, RoutingContext},
        sync::NodeProfile,
    };
    use serde_json::{Value, json};
    let dir = tempfile::tempdir().expect("directory");
    let backend = Arc::new(Backend::default());
    let machine = Machine::open(dir.path(), backend.clone())
        .await
        .expect("machine");
    initialize(&machine).await;
    let context = RoutingContext {
        active_work: vec![],
        recent_activity: vec![],
        request: RouteRequest {
            command_id: Uuid::from_u128(1).try_into().expect("id"),
            thread_id: Uuid::from_u128(2).try_into().expect("id"),
            action: RouteAction::Cancel {},
        },
        candidates: vec![Candidate {
            node_id: Uuid::from_u128(3).try_into().expect("id"),
            profile: NodeProfile {
                platform: Platform::Linux,
                capabilities: Capability::native(),
                capacity: 1,
            },
            available_slots: 1,
        }],
    };
    assert!(machine.choose(&context).await.is_err());
    machine
        .save_setting("model", "not JSON")
        .await
        .expect("corrupt");
    assert!(machine.choose(&context).await.is_err());
    assert!(machine.model_status().await.is_err());
    machine
        .save_setting(
            "model",
            r#"{"endpoint":"http://public.example","model":"model","api_key":"secret"}"#,
        )
        .await
        .expect("unsafe");
    assert!(machine.choose(&context).await.is_err());
    let app=Router::new().route("/",post(|Json(body):Json<Value>|async move{assert_eq!(body["instructions"],"Reviewed local instructions");assert!(body.get("tools").is_none());let node=body["text"]["format"]["schema"]["properties"]["node_id"]["enum"][0].clone();Json(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":json!({"node_id":node}).to_string()}]}]}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    machine
        .configure_model(&ModelSettings {
            endpoint,
            model: "test-model".into(),
            api_key: "private-key".into(),
        })
        .await
        .expect("model");
    let path = dir.path().join("prompts/router/selection.md");
    std::fs::write(&path, "").expect("empty prompt");
    assert!(machine.choose(&context).await.is_err());
    std::fs::write(path, "Reviewed local instructions").expect("prompt");
    assert_eq!(
        machine.choose(&context).await.expect("chosen"),
        context.candidates[0].node_id
    );
    machine.close().await;
    assert!(machine.choose(&context).await.is_err());
    drop(machine);
    let restarted = Machine::open(dir.path(), backend).await.expect("restart");
    assert_eq!(
        restarted
            .choose(&context)
            .await
            .expect("restored model and prompt"),
        context.candidates[0].node_id
    );
    server.abort();
    restarted.close().await;
}
