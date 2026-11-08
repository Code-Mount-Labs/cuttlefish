#![allow(clippy::expect_used)]
use crate::*;
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationHeartbeat, InstallationRole, Registration},
    sync::NodeToken,
};
use uuid::Uuid;

pub(crate) struct Fixture {
    pub(crate) platform: Platform,
    raw: PgPool,
    admin: PgPool,
    schema: String,
    url: String,
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let base = std::env::var("COORDINATOR_TEST_DATABASE_URL")
            .expect("dedicated PostgreSQL test database required");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&base)
            .await
            .expect("test DB");
        let schema = format!("platform_test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let url = url.to_string();
        let platform = Platform::open(&url, "http://127.0.0.1:8080")
            .await
            .expect("platform");
        let raw = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("raw");
        Self {
            platform,
            raw,
            admin,
            schema,
            url,
        }
    }
    pub(crate) async fn account(&self, username: &str) -> NodeToken {
        self.platform
            .register_account(credentials(username))
            .await
            .expect("register")
            .try_into()
            .expect("token")
    }
    async fn group(&self, token: &NodeToken) -> GroupId {
        self.platform
            .create_group(token, "Product team")
            .await
            .expect("group")
            .try_into()
            .expect("ID")
    }
    async fn invitation(&self, token: &NodeToken, group: GroupId) -> NodeToken {
        self.platform
            .invite(token, group)
            .await
            .expect("invite")
            .try_into()
            .expect("token")
    }
    async fn sql(&self, sql: &'static str) {
        sqlx::raw_sql(sql)
            .execute(&self.raw)
            .await
            .expect("test SQL");
    }
    pub(crate) async fn close(self) {
        self.platform.close().await;
        self.raw.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("cleanup");
        self.admin.close().await;
    }
}
pub(crate) fn credentials(username: &str) -> Credentials {
    Credentials {
        username: username.to_owned(),
        password: "test-password-123".to_owned(),
    }
}
fn registration(
    group: GroupId,
    invitation: NodeToken,
    role: InstallationRole,
    seed: u128,
) -> Registration {
    Registration {
        group_id: group,
        installation_id: InstallationId::try_from(Uuid::from_u128(seed)).expect("id"),
        role,
        name: "Workstation".to_owned().try_into().expect("name"),
        router_url: role
            .has_router()
            .then(|| "https://router.example.test".to_owned()),
        credential: format!("{seed:064x}").try_into().expect("credential"),
        invitation,
    }
}
fn heartbeat(role: InstallationRole) -> InstallationHeartbeat {
    InstallationHeartbeat {
        ready: true,
        router_dashboard_url: role
            .has_router()
            .then(|| "https://router.example.test/dashboard".to_owned()),
        agent_dashboard_url: role
            .has_agent()
            .then(|| "http://127.0.0.1:8090/".to_owned()),
    }
}

#[tokio::test]
async fn accounts_sessions_and_group_ownership_survive_replica_replacement() {
    let f = Fixture::new().await;
    let alice = f.account("alice").await;
    let bob = f.account("bob").await;
    let group = f.group(&alice).await;
    assert_eq!(
        f.platform
            .groups(&alice, Uuid::nil())
            .await
            .expect("groups")
            .len(),
        1
    );
    assert!(
        f.platform
            .groups(&bob, Uuid::nil())
            .await
            .expect("isolated")
            .is_empty()
    );
    assert!(f.platform.invite(&bob, group).await.is_err());
    assert!(
        f.platform
            .installations(&bob, group, Uuid::nil())
            .await
            .is_err()
    );
    assert!(
        f.platform
            .register_account(credentials("alice"))
            .await
            .is_err()
    );
    let stored: String =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE username='alice'")
            .fetch_one(&f.raw)
            .await
            .expect("hash");
    assert!(stored.starts_with("$argon2id$"));
    assert!(!stored.contains("test-password"));
    let replica = Platform::open(&f.url, "http://127.0.0.1:8080")
        .await
        .expect("replica");
    assert_eq!(
        replica.account(&alice).await.expect("session"),
        f.platform.account(&alice).await.expect("same account")
    );
    let login: NodeToken = replica
        .login(credentials("alice"))
        .await
        .expect("login")
        .try_into()
        .expect("token");
    assert_eq!(
        replica.account(&login).await.expect("login account"),
        replica.account(&alice).await.expect("original")
    );
    replica.logout(&alice).await.expect("logout");
    replica.logout(&alice).await.expect("retry");
    assert!(f.platform.account(&alice).await.is_err());
    f.sql("UPDATE sessions SET expires_at=clock_timestamp()-interval '1 second'")
        .await;
    assert!(replica.account(&login).await.is_err());
    replica.close().await;
    f.close().await;
}

#[tokio::test]
async fn install_requires_a_router_and_keeps_exactly_one_router_across_restarts() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let invitation = f.invitation(&owner, group).await;
    let options = f
        .platform
        .preflight(group, &invitation)
        .await
        .expect("options");
    assert_eq!(
        options.roles,
        vec![InstallationRole::RouterAgent, InstallationRole::Router]
    );
    let mut first = registration(group, invitation, InstallationRole::Agent, 10);
    assert!(f.platform.register_installation(&first).await.is_err());
    first.role = InstallationRole::RouterAgent;
    first.router_url = Some("https://router.example.test".into());
    assert_eq!(
        f.platform
            .register_installation(&first)
            .await
            .expect("first router"),
        "https://router.example.test"
    );
    assert_eq!(
        f.platform
            .register_installation(&first)
            .await
            .expect("retry"),
        "https://router.example.test"
    );
    let second = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::Router,
        11,
    );
    assert!(f.platform.register_installation(&second).await.is_err());
    let agent = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::Agent,
        12,
    );
    assert!(f.platform.register_installation(&agent).await.is_err());
    f.platform
        .heartbeat(&first.credential, &heartbeat(first.role))
        .await
        .expect("router ready");
    assert_eq!(
        f.platform
            .preflight(group, &agent.invitation)
            .await
            .expect("agent available")
            .roles,
        vec![InstallationRole::Agent]
    );
    f.platform
        .register_installation(&agent)
        .await
        .expect("agent");
    f.platform
        .heartbeat(&agent.credential, &heartbeat(agent.role))
        .await
        .expect("agent ready");
    let replica = Platform::open(&f.url, "http://127.0.0.1:8080")
        .await
        .expect("restart");
    assert!(replica.register_installation(&second).await.is_err());
    check_restarted_topology(&f, &replica, &owner, group, &first, &second, &agent).await;
    replica.close().await;
    f.close().await;
}

async fn check_restarted_topology(
    f: &Fixture,
    replica: &Platform,
    owner: &NodeToken,
    group: GroupId,
    first: &Registration,
    second: &Registration,
    agent: &Registration,
) {
    let listed = replica
        .installations(owner, group, Uuid::nil())
        .await
        .expect("directory");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|entry| entry.online));
    assert_eq!(
        replica
            .authorize_agent(&first.credential, agent.installation_id, &agent.credential)
            .await
            .expect("authorize"),
        group
    );
    assert!(
        replica
            .authorize_agent(&agent.credential, first.installation_id, &first.credential)
            .await
            .is_err()
    );
    f.sql("UPDATE installations SET last_seen=clock_timestamp()-interval '2 minutes'")
        .await;
    assert!(
        replica
            .preflight(group, &second.invitation)
            .await
            .expect("offline retained")
            .roles
            .is_empty()
    );
    assert!(replica.register_installation(second).await.is_err());
    assert!(
        replica
            .installations(owner, group, Uuid::nil())
            .await
            .expect("directory")
            .iter()
            .all(|entry| !entry.online)
    );
}

#[tokio::test]
async fn concurrent_router_claims_have_one_winner_and_changed_retries_fail() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let first = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::Router,
        21,
    );
    let second = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::RouterAgent,
        22,
    );
    let replica = Platform::open(&f.url, "http://127.0.0.1:8080")
        .await
        .expect("replica");
    let (a, b) = tokio::join!(
        f.platform.register_installation(&first),
        replica.register_installation(&second)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let mut winner = if a.is_ok() { first } else { second };
    f.sql("UPDATE invitations SET expires_at=clock_timestamp()-interval '1 second'")
        .await;
    assert!(replica.register_installation(&winner).await.is_ok());
    winner.name = "changed".to_owned().try_into().expect("name");
    assert!(replica.register_installation(&winner).await.is_err());
    assert_eq!(
        replica
            .installations(&owner, group, Uuid::nil())
            .await
            .expect("one router")
            .len(),
        1
    );
    replica.close().await;
    f.close().await;
}

#[test]
fn invalid_urls_and_credentials_never_cross_the_boundary() {
    for invalid in [
        "not a URL",
        "http://example.test",
        "ftp://example.test",
        "https://user@example.test",
        "https://user:pass@example.test",
        "https://example.test?key=x",
        "https://example.test#x",
        "https://example.test/path",
    ] {
        assert!(validate_url(invalid, true).is_err(), "{invalid}");
    }
    for valid in [
        "https://example.test",
        "http://127.0.0.1:8080",
        "http://[::1]:8080/",
    ] {
        assert!(validate_url(valid, true).is_ok());
    }
    assert!(validate_url("https://example.test/dashboard", false).is_ok());
    assert!(!format!("{:?}", credentials("alice")).contains("test-password"));
}

mod http;

mod rejections;

mod faults;

mod addresses;
pub(crate) mod relay;

mod password_change;
