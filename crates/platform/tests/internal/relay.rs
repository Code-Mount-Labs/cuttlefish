use super::*;
use crate::relay::keys::Keys;
use orchestrator_protocol::remote::RelayMutation;
use serde_json::{Value, json};
const PRIVATE: &str = include_str!("../fixtures/public-test-key.pem");
const PUBLIC: &str = include_str!("../fixtures/public-test-key.txt");
pub(crate) fn configured(platform: Platform) -> Platform {
    platform.with_relay(
        Keys::new(
            "http://127.0.0.1:8080",
            "https://relay.test",
            PUBLIC,
            Some(PRIVATE),
        )
        .expect("test keys"),
    )
}
pub(crate) async fn installation(f: &Fixture, seed: u128) -> Registration {
    let account = f.account(&format!("relay.{seed}")).await;
    let group = f.group(&account).await;
    let input = registration(
        group,
        f.invitation(&account, group).await,
        InstallationRole::Router,
        seed,
    );
    f.platform
        .register_installation(&input)
        .await
        .expect("installation");
    input
}
pub(crate) fn mutation(revision: i64) -> RelayMutation {
    RelayMutation {
        revision,
        credential: Some("c".repeat(64)),
    }
}
fn secret() -> NodeToken {
    "c".repeat(64).try_into().expect("secret")
}
fn login(id: InstallationId, token: &str) -> Value {
    json!({"user":id,"privilege_key":token,"run_id":"attacker-chosen","metas":{"untrusted":"ignored"}})
}
async fn reject_invalid_grants(platform: &Platform, own: &Registration) {
    assert!(
        platform
            .change_relay(&NodeToken::random(), &mutation(1))
            .await
            .is_err()
    );
    assert!(
        platform
            .change_relay(&own.credential, &mutation(0))
            .await
            .is_err()
    );
    assert!(
        platform
            .change_relay(
                &own.credential,
                &RelayMutation {
                    revision: 1,
                    credential: Some("bad".into())
                }
            )
            .await
            .is_err()
    );
}
async fn reject_proxy_changes(platform: &Platform, proxy: &Value) {
    for (key, value) in [
        ("proxy_name", json!("victim.dashboard")),
        ("proxy_type", json!("tcp")),
        ("custom_domains", json!(["victim.test"])),
        ("locations", json!(["/elsewhere"])),
    ] {
        let mut attack = proxy.clone();
        attack[key] = value;
        assert!(platform.relay_operation("NewProxy", &attack).await.is_err());
    }
}
async fn reject_identity_changes(platform: &Platform, id: InstallationId, content: &Value) {
    for attack in [
        json!({}),
        json!({"user":{"user":"invalid"}}),
        json!({"user":{"user":id,"metas":{"epoch":Uuid::new_v4()}}}),
        json!({"user":{"user":id,"metas":content["metas"],"run_id":"wrong"}}),
    ] {
        assert!(platform.relay_operation("NewProxy", &attack).await.is_err());
    }
}
fn check_canonical_proxy(canonical: &Value) {
    for key in ["headers", "remote_port", "group"] {
        assert!(canonical["content"].get(key).is_none());
    }
}
#[tokio::test]
async fn grants_are_scoped_idempotent_and_durable_across_restart() {
    let mut f = Fixture::new().await;
    let own = installation(&f, 610).await;
    assert!(
        f.platform
            .change_relay(&own.credential, &mutation(1))
            .await
            .is_err()
    );
    f.platform = configured(f.platform.clone());
    reject_invalid_grants(&f.platform, &own).await;
    let lease = f
        .platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    assert_eq!(lease.installation_id, own.installation_id);
    assert!(
        lease
            .public_url
            .ends_with(&format!("/i/{}", own.installation_id))
    );
    let token = f
        .platform
        .relay_token(own.installation_id, &secret())
        .await
        .expect("token");
    assert!(
        f.platform
            .relay_operation("Login", &login(own.installation_id, &token))
            .await
            .is_ok()
    );
    f.platform.close().await;
    let restarted = configured(
        Platform::open(&f.url, "http://127.0.0.1:8080")
            .await
            .expect("reopen"),
    );
    assert_eq!(
        restarted
            .change_relay(&own.credential, &mutation(1))
            .await
            .expect("retry")
            .epoch,
        lease.epoch
    );
    assert!(
        restarted
            .change_relay(
                &own.credential,
                &RelayMutation {
                    revision: 1,
                    credential: None
                }
            )
            .await
            .is_err()
    );
    let next = restarted
        .change_relay(&own.credential, &mutation(2))
        .await
        .expect("new epoch");
    assert_ne!(next.epoch, lease.epoch);
    assert!(
        restarted
            .relay_operation("Login", &login(own.installation_id, &token))
            .await
            .is_err()
    );
    assert!(
        restarted
            .change_relay(&own.credential, &mutation(1))
            .await
            .is_err()
    );
    let revoked = RelayMutation {
        revision: 3,
        credential: None,
    };
    let stopped = restarted
        .change_relay(&own.credential, &revoked)
        .await
        .expect("revoke");
    assert_eq!(
        restarted
            .change_relay(&own.credential, &revoked)
            .await
            .expect("repeat revoke")
            .epoch,
        stopped.epoch
    );
    assert!(
        restarted
            .relay_token(own.installation_id, &secret())
            .await
            .is_err()
    );
    restarted.close().await;
    f.close().await;
}
#[tokio::test]
async fn hooks_reject_identity_takeover_and_force_only_the_owned_http_route() {
    let mut f = Fixture::new().await;
    let own = installation(&f, 620).await;
    let other = installation(&f, 621).await;
    f.platform = configured(f.platform.clone());
    let lease = f
        .platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    f.platform
        .change_relay(&other.credential, &mutation(1))
        .await
        .expect("other");
    let token = f
        .platform
        .relay_token(own.installation_id, &secret())
        .await
        .expect("token");
    let accepted = f
        .platform
        .relay_operation("Login", &login(own.installation_id, &token))
        .await
        .expect("login");
    let content = &accepted["content"];
    assert_ne!(content["run_id"], "attacker-chosen");
    assert_eq!(content["metas"], json!({"epoch":lease.epoch}));
    assert!(
        f.platform
            .relay_operation("Login", &login(other.installation_id, &token))
            .await
            .is_err()
    );
    let user =
        json!({"user":own.installation_id,"metas":content["metas"],"run_id":content["run_id"]});
    let proxy = json!({"user":user,"proxy_name":format!("{}.{}.dashboard",own.installation_id,lease.epoch),"proxy_type":"http","custom_domains":[format!("{}.{}.cuttlefish.invalid",own.installation_id,lease.epoch)],"locations":["/"],"headers":{"evil":"injection"},"remote_port":1234,"group":"victim"});
    let canonical = f
        .platform
        .relay_operation("NewProxy", &proxy)
        .await
        .expect("route");
    check_canonical_proxy(&canonical);
    reject_proxy_changes(&f.platform, &proxy).await;
    assert!(
        f.platform
            .relay_operation("Ping", &json!({"user":user}))
            .await
            .is_err()
    );
    for operation in ["Ping", "NewWorkConn"] {
        let valid = json!({"user":user,"privilege_key":token});
        assert!(f.platform.relay_operation(operation, &valid).await.is_ok());
        let other_token = f
            .platform
            .relay_token(other.installation_id, &secret())
            .await
            .expect("other token");
        assert!(
            f.platform
                .relay_operation(operation, &json!({"user":user,"privilege_key":other_token}))
                .await
                .is_err()
        );
        assert!(
            f.platform
                .relay_operation(operation, &json!({"user":user,"privilege_key":"invalid"}))
                .await
                .is_err()
        );
    }
    reject_identity_changes(&f.platform, own.installation_id, content).await;
    assert!(
        f.platform
            .relay_operation("Unsupported", &proxy)
            .await
            .is_err()
    );
    assert!(
        f.platform
            .relay_token(own.installation_id, &NodeToken::random())
            .await
            .is_err()
    );
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
    assert!(
        f.platform
            .relay_operation("NewProxy", &proxy)
            .await
            .is_err()
    );
    f.close().await;
}
#[tokio::test]
async fn token_claims_and_operator_configuration_fail_closed() {
    let f = Fixture::new().await;
    assert!(f.platform.clone().configure_relay(None, None, None).is_ok());
    assert!(
        f.platform
            .clone()
            .configure_relay(Some("https://relay.test"), None, None)
            .is_err()
    );
    assert!(
        f.platform
            .clone()
            .configure_relay(Some("invalid"), Some(PUBLIC), Some(PRIVATE))
            .is_err()
    );
    assert!(
        f.platform
            .clone()
            .configure_relay(Some("https://relay.test"), Some(PUBLIC), Some(PRIVATE))
            .is_ok()
    );
    assert!(Keys::new("bad", "https://relay.test", PUBLIC, None).is_err());
    assert!(Keys::new("https://issuer.test", "bad", PUBLIC, None).is_err());
    assert!(Keys::new("https://issuer.test", "https://relay.test", "bad*", None).is_err());
    assert!(
        Keys::new(
            "https://issuer.test",
            "https://relay.test",
            PUBLIC,
            Some("invalid")
        )
        .is_err()
    );
    let keys = Keys::new("http://127.0.0.1:8080", "https://relay.test", PUBLIC, None)
        .expect("verify-only keys");
    assert!(!format!("{keys:?}").contains(PRIVATE));
    let platform = f.platform.clone().with_relay(keys);
    let own = installation(&f, 630).await;
    let lease = platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    assert!(
        platform
            .relay_token(own.installation_id, &secret())
            .await
            .is_err()
    );
    let now = jsonwebtoken::get_current_timestamp();
    for (field, value) in [
        ("exp", json!(1)),
        ("aud", json!("wrong")),
        ("iss", json!("wrong")),
        ("iat", json!(now + 100)),
        ("sub", json!(Uuid::nil())),
        ("epoch", json!(Uuid::nil())),
    ] {
        let mut claims = json!({"sub":own.installation_id,"epoch":lease.epoch,"iss":"http://127.0.0.1:8080","aud":"https://relay.test","iat":now,"exp":now+300});
        claims[field] = value;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA),
            &claims,
            &jsonwebtoken::EncodingKey::from_ed_pem(PRIVATE.as_bytes()).expect("key"),
        )
        .expect("signed token");
        assert!(
            platform
                .relay_operation("Login", &login(own.installation_id, &token))
                .await
                .is_err(),
            "{field}"
        );
    }
    f.close().await;
}

#[path = "relay_http.rs"]
mod http;

#[tokio::test]
async fn publication_requires_the_owned_active_grant_and_database_failures_are_explicit() {
    use orchestrator_protocol::remote::AddressUpdate;
    let mut f = Fixture::new().await;
    let own = installation(&f, 720).await;
    let address = AddressUpdate {
        revision: 1,
        origin: format!("https://relay.test/i/{}", own.installation_id),
    };
    assert!(
        f.platform
            .update_address(&own.credential, &address)
            .await
            .is_err()
    );
    f.platform = configured(f.platform.clone());
    assert!(
        f.platform
            .update_address(&own.credential, &address)
            .await
            .is_err()
    );
    f.platform
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    f.platform
        .update_address(&own.credential, &address)
        .await
        .expect("publish");
    assert!(
        f.platform
            .update_address(
                &own.credential,
                &AddressUpdate {
                    revision: 2,
                    origin: format!("https://relay.test/i/{}", Uuid::new_v4())
                }
            )
            .await
            .is_err()
    );
    let token = f
        .platform
        .relay_token(own.installation_id, &secret())
        .await
        .expect("token");
    for invalid in [
        json!({}),
        json!({"privilege_key":token}),
        json!({"privilege_key":token,"user":"wrong"}),
    ] {
        assert!(f.platform.relay_operation("Login", &invalid).await.is_err());
    }
    assert!(
        Platform::open(&f.url, "http://127.0.0.1:8080")
            .await
            .expect("disabled")
            .relay_operation("Login", &json!({}))
            .await
            .is_err()
    );
    f.sql("ALTER TABLE relay_grants RENAME TO unavailable_grants")
        .await;
    assert!(
        f.platform
            .change_relay(&own.credential, &mutation(2))
            .await
            .is_err()
    );
    assert!(
        f.platform
            .relay_token(own.installation_id, &secret())
            .await
            .is_err()
    );
    assert!(
        f.platform
            .relay_operation("Login", &login(own.installation_id, &token))
            .await
            .is_err()
    );
    f.close().await;
}

#[tokio::test]
async fn disabled_issuer_does_not_mint_an_existing_grant() {
    let f = Fixture::new().await;
    let own = installation(&f, 730).await;
    configured(f.platform.clone())
        .change_relay(&own.credential, &mutation(1))
        .await
        .expect("grant");
    assert!(
        f.platform
            .relay_token(own.installation_id, &secret())
            .await
            .is_err()
    );
    f.close().await;
}
