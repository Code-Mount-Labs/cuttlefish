use super::*;
use orchestrator_protocol::remote::AddressUpdate;

#[tokio::test]
async fn address_revisions_survive_restart() {
    let f = Fixture::new().await;
    let account = f.account("address.owner").await;
    let group = f.group(&account).await;
    let registration = registration(
        group,
        f.invitation(&account, group).await,
        InstallationRole::RouterAgent,
        501,
    );
    f.platform
        .register_installation(&registration)
        .await
        .expect("register");
    let credential = &registration.credential;
    let update = AddressUpdate {
        revision: 1,
        origin: "https://new.example.test".into(),
    };
    f.platform
        .update_address(credential, &update)
        .await
        .expect("publish");
    f.platform
        .heartbeat(credential, &heartbeat(registration.role))
        .await
        .expect("delayed heartbeat");
    let view = f.platform.installation(credential).await.expect("view");
    assert_eq!(
        view.router_dashboard_url.as_deref(),
        Some(update.origin.as_str())
    );
    assert_eq!(
        view.agent_dashboard_url.as_deref(),
        Some(update.origin.as_str())
    );
    f.platform.close().await;
    let reopened = Platform::open(&f.url, "http://127.0.0.1:8080")
        .await
        .expect("restart");
    reopened
        .update_address(credential, &update)
        .await
        .expect("lost response retry");
    assert_eq!(
        reopened.router_address(credential).await.expect("router"),
        update.origin
    );
    assert!(
        reopened
            .update_address(
                credential,
                &AddressUpdate {
                    origin: "https://other.test".into(),
                    ..update.clone()
                }
            )
            .await
            .is_err()
    );
    let newer = AddressUpdate {
        revision: 2,
        origin: "http://127.0.0.1:8080".into(),
    };
    reopened
        .update_address(credential, &newer)
        .await
        .expect("return local");
    assert!(reopened.update_address(credential, &update).await.is_err());
    assert_eq!(
        reopened.router_address(credential).await.expect("router"),
        newer.origin
    );
    reopened.close().await;
    f.close().await;
}

#[tokio::test]
async fn address_endpoints_require_installation_authentication() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let account = f.account("address.http").await;
    let group = f.group(&account).await;
    let registration = registration(
        group,
        f.invitation(&account, group).await,
        InstallationRole::Router,
        502,
    );
    f.platform
        .register_installation(&registration)
        .await
        .expect("register");
    let app = web::router(f.platform.clone());
    for (path, body) in [
        (
            "/api/v1/installations/address",
            r#"{"revision":1,"origin":"https://remote.test"}"#,
        ),
        ("/api/v1/installations/router-address", "{}"),
    ] {
        let unknown = NodeToken::random();
        for (credential, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(unknown.expose()), StatusCode::UNAUTHORIZED),
            (Some(registration.credential.expose()), StatusCode::OK),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json");
            if let Some(credential) = credential {
                request = request.header("authorization", format!("Bearer {credential}"));
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).expect("request"))
                .await
                .expect("response");
            assert_eq!(response.status(), expected);
        }
    }
    f.close().await;
}

#[tokio::test]
async fn address_storage_failures_preserve_the_previous_public_address() {
    let f = Fixture::new().await;
    let account = f.account("address.faults").await;
    let group = f.group(&account).await;
    let registration = registration(
        group,
        f.invitation(&account, group).await,
        InstallationRole::Router,
        503,
    );
    f.platform
        .register_installation(&registration)
        .await
        .expect("register");
    let update = AddressUpdate {
        revision: 1,
        origin: "https://next.test".into(),
    };
    f.sql("ALTER TABLE installations RENAME COLUMN address_origin TO unavailable_origin")
        .await;
    assert!(
        f.platform
            .update_address(&registration.credential, &update)
            .await
            .is_err()
    );
    f.sql("ALTER TABLE installations RENAME COLUMN unavailable_origin TO address_origin")
        .await;
    f.sql("CREATE FUNCTION reject_address_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected write failure'; END $$; CREATE TRIGGER reject_write BEFORE UPDATE ON installations FOR EACH ROW EXECUTE FUNCTION reject_address_write()").await;
    assert!(
        f.platform
            .update_address(&registration.credential, &update)
            .await
            .is_err()
    );
    f.sql("DROP TRIGGER reject_write ON installations; CREATE CONSTRAINT TRIGGER reject_commit AFTER UPDATE ON installations DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_address_write()").await;
    assert!(
        f.platform
            .update_address(&registration.credential, &update)
            .await
            .is_err()
    );
    assert_eq!(
        f.platform
            .router_address(&registration.credential)
            .await
            .expect("unchanged"),
        "https://router.example.test"
    );
    f.sql("DROP TRIGGER reject_commit ON installations").await;
    f.platform
        .update_address(&registration.credential, &update)
        .await
        .expect("retry succeeds");
    f.platform.close().await;
    assert!(
        f.platform
            .update_address(&registration.credential, &update)
            .await
            .is_err()
    );
    assert!(
        f.platform
            .router_address(&registration.credential)
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn address_changes_preserve_group_isolation() {
    let f = Fixture::new().await;
    let owner = f.account("address.isolation").await;
    let group = f.group(&owner).await;
    let other_group = f.group(&owner).await;
    let router = registered(&f, &owner, group, InstallationRole::Router, 504).await;
    f.platform
        .heartbeat(&router.credential, &heartbeat(router.role))
        .await
        .expect("ready router");
    let agent = registered(&f, &owner, group, InstallationRole::Agent, 505).await;
    let other = registered(&f, &owner, other_group, InstallationRole::Router, 506).await;
    let credential = &agent.credential;
    let update = AddressUpdate {
        revision: 1,
        origin: "https://agent.test".into(),
    };
    f.platform
        .update_address(credential, &update)
        .await
        .expect("publish agent");
    let view = f
        .platform
        .installation(credential)
        .await
        .expect("agent view");
    assert_eq!(
        view.agent_dashboard_url.as_deref(),
        Some(update.origin.as_str())
    );
    assert!(view.router_dashboard_url.is_none());
    assert_eq!(
        f.platform
            .router_address(credential)
            .await
            .expect("own router"),
        "https://router.example.test"
    );
    let other_update = AddressUpdate {
        revision: 1,
        origin: "https://other-group.test".into(),
    };
    f.platform
        .update_address(&other.credential, &other_update)
        .await
        .expect("other group publish");
    assert_eq!(
        f.platform
            .router_address(credential)
            .await
            .expect("own router unchanged"),
        "https://router.example.test"
    );
    assert_eq!(
        f.platform
            .router_address(&other.credential)
            .await
            .expect("other router"),
        other_update.origin
    );
    assert!(
        f.platform
            .installation(&other.credential)
            .await
            .expect("router view")
            .agent_dashboard_url
            .is_none()
    );
    f.close().await;
}

async fn registered(
    f: &Fixture,
    owner: &NodeToken,
    group: GroupId,
    role: InstallationRole,
    seed: u128,
) -> Registration {
    let input = registration(group, f.invitation(owner, group).await, role, seed);
    f.platform
        .register_installation(&input)
        .await
        .expect("register");
    input
}

#[tokio::test]
async fn address_changes_reject_unknown_credentials_and_invalid_input() {
    let f = Fixture::new().await;
    let owner = f.account("address.validation").await;
    let group = f.group(&owner).await;
    let router = registered(&f, &owner, group, InstallationRole::Router, 507).await;
    let credential = &router.credential;
    let update = AddressUpdate {
        revision: 1,
        origin: "https://valid.test".into(),
    };
    let unknown = NodeToken::random();
    assert!(matches!(
        f.platform.update_address(&unknown, &update).await,
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        f.platform.router_address(&unknown).await,
        Err(Error::Unauthorized)
    ));
    assert!(
        f.platform
            .update_address(
                credential,
                &AddressUpdate {
                    revision: 0,
                    ..update.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(
        f.platform
            .update_address(
                credential,
                &AddressUpdate {
                    origin: "http://unsafe.test".into(),
                    ..update.clone()
                }
            )
            .await
            .is_err()
    );

    f.close().await;
}
