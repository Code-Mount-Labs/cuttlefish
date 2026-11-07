//! Provider-neutral remote-access wire compatibility.
#![allow(clippy::expect_used)]
use orchestrator_protocol::remote::{AddressUpdate, RemoteChoice};
#[test]
fn remote_contracts_round_trip() {
    for choice in [
        RemoteChoice::Local {},
        RemoteChoice::Managed {},
        RemoteChoice::Existing {
            origin: "https://node.example.com".into(),
        },
    ] {
        let wire = serde_json::to_string(&choice).expect("encode");
        assert_eq!(
            serde_json::from_str::<RemoteChoice>(&wire).expect("decode"),
            choice
        );
        assert!(!format!("{choice:?}").is_empty());
    }
    for invalid in [
        r#"{"mode":"local","unexpected":true}"#,
        r#"{"mode":"managed","unexpected":true}"#,
        r#"{"mode":"existing","origin":"https://example.com","unexpected":true}"#,
        r#"{"mode":"unknown"}"#,
        r#"{"mode":"existing"}"#,
    ] {
        assert!(serde_json::from_str::<RemoteChoice>(invalid).is_err());
    }
    let address = AddressUpdate {
        revision: 1,
        origin: "https://node.example.com".into(),
    };
    let restored =
        serde_json::from_str::<AddressUpdate>(&serde_json::to_string(&address).expect("encode"))
            .expect("decode");
    assert_eq!(restored.revision, address.revision);
    assert_eq!(restored.origin, address.origin);
}

#[test]
fn endpoints_accept_only_canonical_installation_paths_and_safe_origins() {
    use orchestrator_protocol::remote::canonical_endpoint;
    let id = "e891fd38-4015-45bb-bb08-46da92867e76";
    for value in [
        "https://example.com",
        "http://127.0.0.1:8080",
        "http://[::1]:8080",
    ] {
        assert_eq!(canonical_endpoint(value).as_deref(), Some(value));
        assert_eq!(
            canonical_endpoint(&format!("{value}/")).as_deref(),
            Some(value)
        );
        let scoped = format!("{value}/i/{id}");
        assert_eq!(
            canonical_endpoint(&scoped).as_deref(),
            Some(scoped.as_str())
        );
    }
    for value in [
        "invalid",
        "file:///tmp/a",
        "mailto:operator@example.com",
        "http://example.com",
        "https://user@example.com",
        "https://user:pass@example.com",
        "https://example.com?q=1",
        "https://example.com#fragment",
        "https://example.com/a",
        "https://example.com//",
        "https://example.com/a/../",
        "https://example.com/./",
        "https://example.com/%69/abc",
        "https://example.com/\\bad",
        "https://example.com/i/no",
        "https://example.com/i/00000000-0000-0000-0000-000000000000",
        "https://example.com/i/E891FD38-4015-45BB-BB08-46DA92867E76",
        "https://example.com/i/e891fd38401545bbbb0846da92867e76",
        "https://example.com/i/e891fd38-4015-45bb-bb08-46da92867e76//",
    ] {
        assert!(canonical_endpoint(value).is_none(), "{value}");
    }
}
#[test]
fn connector_credentials_are_redacted_and_leases_round_trip() {
    use orchestrator_protocol::{
        ids::InstallationId,
        remote::{RelayLease, RelayMutation},
    };
    let mutation = RelayMutation {
        revision: 1,
        credential: Some("private-credential".into()),
    };
    assert!(!format!("{mutation:?}").contains("private-credential"));
    let restored: RelayMutation =
        serde_json::from_value(serde_json::to_value(&mutation).expect("serialize"))
            .expect("decode");
    assert_eq!(restored.credential, mutation.credential);
    let lease = RelayLease {
        installation_id: InstallationId::random(),
        epoch: uuid::Uuid::new_v4(),
        public_url: "https://relay.example.com/i/id".into(),
        issuer: "https://platform.example.com".into(),
    };
    let restored: RelayLease =
        serde_json::from_value(serde_json::to_value(&lease).expect("serialize")).expect("decode");
    assert_eq!(restored.installation_id, lease.installation_id);
}
