//! Installation wire compatibility and credential redaction.
#![allow(clippy::expect_used)]
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationRole, Registration},
};
use uuid::Uuid;
#[test]
fn registration_round_trips_and_rejects_invalid_credentials() {
    for (role, router, agent) in [
        (InstallationRole::RouterAgent, true, true),
        (InstallationRole::Router, true, false),
        (InstallationRole::Agent, false, true),
    ] {
        assert_eq!(role.has_router(), router);
        assert_eq!(role.has_agent(), agent);
        let input = Registration {
            group_id: GroupId::try_from(Uuid::from_u128(1)).expect("group"),
            installation_id: InstallationId::try_from(Uuid::from_u128(2)).expect("installation"),
            role,
            name: "Laptop".to_owned().try_into().expect("name"),
            router_url: router.then(|| "https://router.test".into()),
            credential: "a".repeat(64).try_into().expect("token"),
            invitation: "b".repeat(64).try_into().expect("invitation"),
        };
        assert!(!format!("{input:?}").contains(&"a".repeat(64)));
        let mut wire = serde_json::to_value(&input).expect("serialize");
        let saved: Registration = serde_json::from_value(wire.clone()).expect("deserialize");
        assert_eq!(saved.credential.expose(), input.credential.expose());
        wire["credential"] = serde_json::json!("bad");
        assert!(serde_json::from_value::<Registration>(wire.clone()).is_err());
        wire["credential"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Registration>(wire).is_err());
    }
}
