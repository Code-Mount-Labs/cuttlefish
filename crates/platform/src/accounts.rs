use super::{Error, Platform, digest, random_token, write_error};
use orchestrator_protocol::sync::NodeToken;
use serde::Deserialize;
use uuid::Uuid;

/// Username/password input; credentials never appear in Debug output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    /// Account's lowercase username.
    pub username: String,
    /// Unmodified password, 12–1024 bytes.
    pub password: String,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}

impl Platform {
    /// Create an account and durable expiring session atomically.
    ///
    /// # Errors
    /// Rejects invalid/duplicate credentials, hashing limits and storage failures.
    pub async fn register_account(&self, credentials: Credentials) -> Result<String, Error> {
        validate(&credentials)?;
        let permit = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limited)?;
        let (username, hash) = self.passwords.hash(credentials, permit).await?;
        let account = Uuid::new_v4();
        let token = random_token();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO accounts(account_id,username,password_hash) VALUES($1,$2,$3)")
            .bind(account)
            .bind(username)
            .bind(hash)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        insert_session(&mut tx, account, &token).await?;
        tx.commit().await?;
        Ok(token)
    }

    /// Authenticate and create an eight-hour session for web or mobile clients.
    /// Attempt reservations are shared across replicas and survive restart.
    ///
    /// # Errors
    /// Reports invalid credentials, rate limits and unavailable storage.
    pub async fn login(&self, credentials: Credentials) -> Result<String, Error> {
        validate(&credentials)?;
        let permit = self
            .password_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limited)?;
        let row:Option<(Uuid,String)>=sqlx::query_as("UPDATE accounts SET failed_logins=CASE WHEN login_window<clock_timestamp()-interval '1 minute' THEN 1 ELSE failed_logins+1 END,login_window=CASE WHEN login_window<clock_timestamp()-interval '1 minute' THEN clock_timestamp() ELSE login_window END WHERE username=$1 AND (failed_logins<5 OR login_window<clock_timestamp()-interval '1 minute') RETURNING account_id,password_hash")
            .bind(&credentials.username).fetch_optional(&self.pool).await?;
        let (account, hash) = row.ok_or(Error::Unauthorized)?;
        let valid = self
            .passwords
            .verify(credentials.password, hash, permit)
            .await?;
        if !valid {
            return Err(Error::Unauthorized);
        }
        let token = random_token();
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE accounts SET failed_logins=0,login_window=clock_timestamp() WHERE account_id=$1").bind(account).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM sessions WHERE account_id=$1 AND expires_at<=clock_timestamp()")
            .bind(account)
            .execute(&mut *tx)
            .await?;
        insert_session(&mut tx, account, &token).await?;
        tx.commit().await?;
        Ok(token)
    }

    /// Resolve a session to its account. Node/installation tokens cannot sign in.
    ///
    /// # Errors
    /// Rejects unknown or expired credentials, or unavailable storage.
    pub async fn account(&self, token: &NodeToken) -> Result<Uuid, Error> {
        let account: Option<Uuid> = sqlx::query_scalar(
            "SELECT account_id FROM sessions WHERE token_hash=$1 AND expires_at>clock_timestamp()",
        )
        .bind(digest(token.expose()))
        .fetch_optional(&self.pool)
        .await?;
        account.ok_or(Error::Unauthorized)
    }

    /// Revoke exactly this session on all platform replicas.
    ///
    /// # Errors
    /// Reports unavailable storage.
    pub async fn logout(&self, token: &NodeToken) -> Result<(), Error> {
        sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
            .bind(digest(token.expose()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn insert_session(
    connection: &mut sqlx::PgConnection,
    account: Uuid,
    token: &str,
) -> Result<(), Error> {
    sqlx::query("INSERT INTO sessions(token_hash,account_id,expires_at) VALUES($1,$2,clock_timestamp()+interval '8 hours')")
        .bind(digest(token)).bind(account).execute(connection).await?;
    Ok(())
}

fn validate(credentials: &Credentials) -> Result<(), Error> {
    if !(3..=64).contains(&credentials.username.len())
        || !credentials
            .username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    {
        return Err(Error::Invalid(
            "username must contain 3–64 lowercase letters, digits, dots, underscores or hyphens",
        ));
    }
    if !(12..=1024).contains(&credentials.password.len()) {
        return Err(Error::Invalid("password must contain 12–1024 bytes"));
    }
    Ok(())
}
