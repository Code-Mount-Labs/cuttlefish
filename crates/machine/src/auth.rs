//! Machine-local administrator authentication works before `PostgreSQL` exists.
use crate::{Error, state::Machine};
use orchestrator_protocol::sync::NodeToken;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Administrator credentials, redacted in diagnostic output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    /// Local username.
    pub username: String,
    /// Unmodified password.
    pub password: String,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}
impl Machine {
    /// Whether the local administrator has completed first-time setup.
    ///
    /// # Errors
    /// Reports local storage failure.
    pub async fn initialized(&self) -> Result<bool, Error> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_admin)")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    /// Read or create the local first-setup secret. It never grants access after
    /// an administrator exists, even if this file is retained in a backup.
    ///
    /// # Errors
    /// Reports protected-file creation failures.
    pub fn bootstrap_token(&self) -> Result<NodeToken, Error> {
        let path = self.directory().join("setup.token");
        if !path.exists() {
            self.files
                .write_new(self.directory(), &path, random_token().as_bytes())?;
        }
        NodeToken::try_from(std::fs::read_to_string(path)?)
            .map_err(|_| Error::Invalid("invalid_local_setup_token"))
    }
    /// Create exactly one administrator with the locally generated setup secret.
    ///
    /// # Errors
    /// Rejects invalid secrets, repeated setup, invalid input or storage failures.
    pub async fn setup_admin(
        &self,
        token: &NodeToken,
        input: Credentials,
    ) -> Result<String, Error> {
        if digest(token.expose()) != digest(self.bootstrap_token()?.expose()) {
            return Err(Error::Unauthorized);
        }
        validate(&input)?;
        let permit = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limited)?;
        let (username, hash) = self.passwords.hash(input, permit).await?;
        let mut tx = self.pool.begin().await?;
        let created=sqlx::query("INSERT INTO local_admin(singleton,username,password_hash) VALUES(1,?1,?2) ON CONFLICT DO NOTHING").bind(username).bind(hash).execute(&mut *tx).await?.rows_affected();
        if created == 0 {
            return Err(Error::Invalid("administrator_already_configured"));
        }
        let session = random_token();
        sqlx::query(
            "INSERT INTO local_sessions(token_hash,expires_at) VALUES(?1,unixepoch()+28800)",
        )
        .bind(digest(&session))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(session)
    }
    /// Sign in using local credentials. Attempt counters survive process restart.
    ///
    /// # Errors
    /// Rejects incorrect credentials, attempt limits and unavailable storage.
    pub async fn login(&self, input: Credentials) -> Result<String, Error> {
        validate(&input)?;
        let permit = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limited)?;
        let row:Option<(String,String)>=sqlx::query_as("UPDATE local_admin SET failed_logins=CASE WHEN login_window<unixepoch()-60 THEN 1 ELSE failed_logins+1 END,login_window=CASE WHEN login_window<unixepoch()-60 THEN unixepoch() ELSE login_window END WHERE failed_logins<5 OR login_window<unixepoch()-60 RETURNING username,password_hash").fetch_optional(&self.pool).await?;
        let (username, hash) = row.ok_or(Error::Unauthorized)?;
        let username_matches = input.username == username;
        let valid = self.passwords.verify(input.password, hash, permit).await?;
        if !valid || !username_matches {
            return Err(Error::Unauthorized);
        }
        self.issue_login_session().await
    }
    async fn issue_login_session(&self) -> Result<String, Error> {
        let token = random_token();
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE local_admin SET failed_logins=0,login_window=unixepoch()")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM local_sessions WHERE expires_at<=unixepoch()")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO local_sessions(token_hash,expires_at) VALUES(?1,unixepoch()+28800)",
        )
        .bind(digest(&token))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(token)
    }
    /// Authenticate an unexpired local session.
    ///
    /// # Errors
    /// Rejects unknown/expired sessions and storage failure.
    pub async fn authenticate(&self, token: &NodeToken) -> Result<(), Error> {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_sessions WHERE token_hash=?1 AND expires_at>unixepoch())").bind(digest(token.expose())).fetch_one(&self.pool).await?;
        if !valid {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    /// Revoke a local administrator session immediately.
    ///
    /// # Errors
    /// Reports local storage failure.
    pub async fn logout(&self, token: &NodeToken) -> Result<(), Error> {
        sqlx::query("DELETE FROM local_sessions WHERE token_hash=?1")
            .bind(digest(token.expose()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
fn validate(input: &Credentials) -> Result<(), Error> {
    if !(3..=64).contains(&input.username.len())
        || !input
            .username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        || !(12..=1024).contains(&input.password.len())
    {
        return Err(Error::Invalid(
            "use a lowercase username (3–64 characters) and a password of 12–1024 bytes",
        ));
    }
    Ok(())
}
pub(crate) fn digest(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}
pub(crate) fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[cfg(test)]
#[path = "../tests/internal/auth.rs"]
mod tests;
