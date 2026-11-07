use super::{Error, Platform, digest, random_token, validate_url, write_error};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationHeartbeat, InstallationRole, InstallationView, Registration},
    sync::NodeToken,
};
use serde::Serialize;
use sqlx::{PgConnection, types::Json};
use uuid::Uuid;

/// Account-owned group summary. Router identity remains reserved while offline.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GroupView {
    /// Stable group identity used by the installer.
    pub group_id: Uuid,
    /// Human-readable group name.
    pub name: String,
    /// Number of installed agents, including combined installations.
    pub agents: i64,
    /// Number of fresh, ready agent installations.
    pub online: i64,
    /// Whether the one router slot is already occupied.
    pub has_router: bool,
}

/// Authenticated installation options, derived from durable topology.
#[derive(Debug, Serialize)]
pub struct Topology {
    /// Roles the next installation may claim now.
    pub roles: Vec<InstallationRole>,
    /// Current group's router, even if not presently ready.
    pub router_url: Option<String>,
}

const VIEW: &str = "SELECT json_build_object('installation_id',installation_id,'group_id',group_id,'role',role,'name',name,'router_url',router_url,'router_dashboard_url',router_dashboard_url,'agent_dashboard_url',agent_dashboard_url,'online',COALESCE(ready AND last_seen>clock_timestamp()-interval '60 seconds',FALSE)) FROM installations";

impl Platform {
    /// Create an account-owned group before any machine has been installed.
    ///
    /// # Errors
    /// Rejects unauthenticated callers, invalid names and storage failure.
    pub async fn create_group(&self, token: &NodeToken, name: &str) -> Result<Uuid, Error> {
        let account = self.account(token).await?;
        validate_name(name)?;
        let group = Uuid::new_v4();
        sqlx::query("INSERT INTO app_groups(group_id,account_id,name) VALUES($1,$2,$3)")
            .bind(group)
            .bind(account)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(group)
    }

    /// Page the signed-in account's groups, never other users' groups.
    ///
    /// # Errors
    /// Requires a valid session and available storage.
    pub async fn groups(&self, token: &NodeToken, after: Uuid) -> Result<Vec<GroupView>, Error> {
        let account = self.account(token).await?;
        Ok(sqlx::query_as("SELECT g.group_id,g.name,count(i.installation_id) FILTER(WHERE i.role IN ('agent','router_agent')) agents,count(i.installation_id) FILTER(WHERE i.role IN ('agent','router_agent') AND i.ready AND i.last_seen>clock_timestamp()-interval '60 seconds') online,COALESCE(bool_or(i.role IN ('router','router_agent')),FALSE) has_router FROM app_groups g LEFT JOIN installations i USING(group_id) WHERE g.account_id=$1 AND g.group_id>$2 GROUP BY g.group_id,g.name ORDER BY g.group_id LIMIT 100")
            .bind(account).bind(after).fetch_all(&self.pool).await?)
    }

    /// Page a group's installations and their separate dashboard URLs.
    ///
    /// # Errors
    /// Rejects cross-account access and storage failure.
    pub async fn installations(
        &self,
        token: &NodeToken,
        group: GroupId,
        after: Uuid,
    ) -> Result<Vec<InstallationView>, Error> {
        self.owns(token, group).await?;
        let rows: Vec<Json<InstallationView>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "{VIEW} WHERE group_id=$1 AND installation_id>$2 ORDER BY installation_id LIMIT 100"
        )))
        .bind(Uuid::from(group))
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    /// Issue a one-installation invitation, valid for thirty minutes. The group
    /// ID is an identifier, never a substitute for this authorization.
    ///
    /// # Errors
    /// Requires group ownership and available storage.
    pub async fn invite(&self, token: &NodeToken, group: GroupId) -> Result<String, Error> {
        self.owns(token, group).await?;
        let invitation = random_token();
        sqlx::query("INSERT INTO invitations(token_hash,group_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '30 minutes')")
            .bind(digest(&invitation)).bind(Uuid::from(group)).execute(&self.pool).await?;
        Ok(invitation)
    }

    /// Show available installer roles without exposing a group to anonymous lookup.
    ///
    /// # Errors
    /// Rejects invalid/expired invitations and unavailable storage.
    pub async fn preflight(
        &self,
        group: GroupId,
        invitation: &NodeToken,
    ) -> Result<Topology, Error> {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM invitations WHERE group_id=$1 AND token_hash=$2 AND expires_at>clock_timestamp() AND registration_hash IS NULL)")
            .bind(Uuid::from(group)).bind(digest(invitation.expose())).fetch_one(&self.pool).await?;
        if !valid {
            return Err(Error::Unauthorized);
        }
        topology(&mut *self.pool.acquire().await?, group).await
    }

    /// Claim a role atomically. A group row lock and unique database constraint
    /// prevent two routers even when installation requests race across replicas.
    ///
    /// # Errors
    /// Rejects changed retries, invalid invitations, duplicate routers and agents
    /// without a ready router. No failure authorizes a replacement router.
    pub async fn register_installation(&self, input: &Registration) -> Result<String, Error> {
        validate_name(input.name.as_str())?;
        let router = input
            .router_url
            .as_deref()
            .map(|url| validate_url(url, true))
            .transpose()?;
        if input.role.has_router() != router.is_some() {
            return Err(Error::Invalid(
                "router roles require a router URL; agent-only roles must omit it",
            ));
        }
        let fingerprint = registration_digest(input);
        let invite = digest(input.invitation.expose());
        let mut tx = self.pool.begin().await?;
        let saved:Option<(bool,Option<Vec<u8>>)>=sqlx::query_as("SELECT expires_at>clock_timestamp(),registration_hash FROM invitations WHERE group_id=$1 AND token_hash=$2 FOR UPDATE")
            .bind(Uuid::from(input.group_id)).bind(&invite).fetch_optional(&mut *tx).await?;
        let (valid, previous) = saved.ok_or(Error::Unauthorized)?;
        if let Some(previous) = previous {
            if previous != fingerprint {
                return Err(Error::Conflict(
                    "invitation was already used by another registration",
                ));
            }
            return topology(&mut tx, input.group_id)
                .await?
                .router_url
                .ok_or(Error::Conflict("registered router is missing"));
        }
        if !valid {
            return Err(Error::Unauthorized);
        }
        sqlx::query("SELECT group_id FROM app_groups WHERE group_id=$1 FOR UPDATE")
            .bind(Uuid::from(input.group_id))
            .fetch_one(&mut *tx)
            .await?;
        let existing = topology(&mut tx, input.group_id).await?;
        let router_url = router
            .clone()
            .or(existing.router_url)
            .filter(|_| existing.roles.contains(&input.role))
            .ok_or(Error::Conflict(
                "role unavailable: groups need exactly one router before adding agents",
            ))?;
        sqlx::query("INSERT INTO installations(installation_id,group_id,role,name,token_hash,router_url) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(Uuid::from(input.installation_id)).bind(Uuid::from(input.group_id)).bind(role_name(input.role)).bind(input.name.as_str()).bind(digest(input.credential.expose())).bind(&router).execute(&mut *tx).await.map_err(write_error)?;
        sqlx::query("UPDATE invitations SET registration_hash=$1 WHERE token_hash=$2")
            .bind(fingerprint)
            .bind(invite)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(router_url)
    }

    /// Read the caller's own registration. This is safe to retry after installation.
    ///
    /// # Errors
    /// Rejects unknown installation credentials and storage failure.
    pub async fn installation(&self, token: &NodeToken) -> Result<InstallationView, Error> {
        let row: Option<Json<InstallationView>> =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("{VIEW} WHERE token_hash=$1")))
                .bind(digest(token.expose()))
                .fetch_optional(&self.pool)
                .await?;
        row.map(|row| row.0).ok_or(Error::Unauthorized)
    }

    /// Update only the authenticated installation's liveness and dashboard links.
    /// Registry liveness never authorizes replay or reassignment of execution.
    ///
    /// # Errors
    /// Rejects invalid addresses, wrong-role dashboards and unavailable storage.
    pub async fn heartbeat(
        &self,
        token: &NodeToken,
        input: &InstallationHeartbeat,
    ) -> Result<(), Error> {
        let own = self.installation(token).await?;
        let router = input
            .router_dashboard_url
            .as_deref()
            .map(|url| validate_url(url, false))
            .transpose()?;
        let agent = input
            .agent_dashboard_url
            .as_deref()
            .map(|url| validate_url(url, false))
            .transpose()?;
        if router.is_some() && !own.role.has_router() || agent.is_some() && !own.role.has_agent() {
            return Err(Error::Invalid("dashboard does not match installation role"));
        }
        sqlx::query("UPDATE installations SET ready=$1,last_seen=clock_timestamp(),router_dashboard_url=$2,agent_dashboard_url=$3 WHERE installation_id=$4")
            .bind(input.ready).bind(router).bind(agent).bind(Uuid::from(own.installation_id)).execute(&self.pool).await?;
        Ok(())
    }

    /// Allow a registered router to verify an agent credential in its own group.
    /// A different group's agent or a router-only installation cannot enroll.
    ///
    /// # Errors
    /// Rejects cross-group or wrong-role credentials and storage failure.
    pub async fn authorize_agent(
        &self,
        router: &NodeToken,
        agent: InstallationId,
        credential: &NodeToken,
    ) -> Result<GroupId, Error> {
        let router = self.installation(router).await?;
        let candidate = self.installation(credential).await?;
        if !router.role.has_router()
            || !candidate.role.has_agent()
            || candidate.installation_id != agent
            || candidate.group_id != router.group_id
        {
            return Err(Error::Unauthorized);
        }
        Ok(router.group_id)
    }

    async fn owns(&self, token: &NodeToken, group: GroupId) -> Result<(), Error> {
        let account = self.account(token).await?;
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM app_groups WHERE group_id=$1 AND account_id=$2)",
        )
        .bind(Uuid::from(group))
        .bind(account)
        .fetch_one(&self.pool)
        .await?;
        if !valid {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
}

async fn topology(connection: &mut PgConnection, group: GroupId) -> Result<Topology, Error> {
    let router:Option<(String,bool)>=sqlx::query_as("SELECT router_url,COALESCE(ready AND last_seen>clock_timestamp()-interval '60 seconds',FALSE) FROM installations WHERE group_id=$1 AND role IN ('router','router_agent')")
        .bind(Uuid::from(group)).fetch_optional(connection).await?;
    Ok(match router {
        None => Topology {
            roles: vec![InstallationRole::RouterAgent, InstallationRole::Router],
            router_url: None,
        },
        Some((url, ready)) => Topology {
            roles: if ready {
                vec![InstallationRole::Agent]
            } else {
                vec![]
            },
            router_url: Some(url),
        },
    })
}

fn role_name(role: InstallationRole) -> &'static str {
    match role {
        InstallationRole::RouterAgent => "router_agent",
        InstallationRole::Router => "router",
        InstallationRole::Agent => "agent",
    }
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "name must contain 1–128 bytes without control characters",
        ));
    }
    Ok(())
}

fn registration_digest(input: &Registration) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"cuttlefish.registration.v1\0");
    for field in [
        input.group_id.to_string(),
        input.installation_id.to_string(),
        role_name(input.role).to_owned(),
        input.name.as_str().to_owned(),
        input.router_url.clone().unwrap_or_default(),
        input.credential.expose().to_owned(),
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field.as_bytes());
    }
    hash.finalize().to_vec()
}
