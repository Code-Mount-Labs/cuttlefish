//! Hosted account and installation registry. This service never routes tasks or
//! executes agents; each group owns a separate installed routing service.

mod accounts;
mod addresses;
mod passwords;
mod registry;
pub mod relay;
pub mod web;

use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{sync::Arc, time::Duration};
use thiserror::Error;

/// Account/group registry backed by shared `PostgreSQL`.
#[derive(Debug, Clone)]
pub struct Platform {
    pool: PgPool,
    relay_keys: Option<Arc<relay::keys::Keys>>,
    origin: String,
    password_slots: Arc<tokio::sync::Semaphore>,
    passwords: passwords::Passwords,
}

/// Public failure categories; no secrets or database internals reach clients.
#[derive(Debug, Error)]
pub enum Error {
    /// Redacted relay or upstream service failure.
    #[error("{0}")]
    Transport(&'static str),
    /// Invalid bounded input.
    #[error("{0}")]
    Invalid(&'static str),
    /// Missing, expired or cross-account credentials.
    #[error("authentication required")]
    Unauthorized,
    /// Registration or topology conflicts with a durable record.
    #[error("{0}")]
    Conflict(&'static str),
    /// Password work or attempt limits have been reached.
    #[error("too many attempts; try again later")]
    Limited,
    /// Storage failure, without exposing connection details.
    #[error("platform storage unavailable")]
    Database(#[from] sqlx::Error),
    /// Migration failed.
    #[error("platform migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// Password worker failed.
    #[error("password service unavailable")]
    Worker(#[from] tokio::task::JoinError),
    /// Hashing failed before any account was created.
    #[error("password hashing unavailable")]
    Password,
}

impl Platform {
    /// Open the hosted registry and apply its independent migration history.
    ///
    /// # Errors
    /// Reports invalid public origins, database connection and migration failures.
    pub async fn open(database_url: &str, origin: &str) -> Result<Self, Error> {
        let origin = validate_url(origin, true)?;
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self {
            pool,
            relay_keys: None,
            origin,
            password_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            passwords: passwords::Passwords::default(),
        })
    }
    /// Close this process's registry connections.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn validate_url(value: &str, origin_only: bool) -> Result<String, Error> {
    let url = reqwest::Url::parse(value).map_err(|_| Error::Invalid("invalid URL"))?;
    let local = url
        .host_str()
        .and_then(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|ip| ip.is_loopback());
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || origin_only && url.path() != "/"
    {
        return Err(Error::Invalid(
            "URL must use HTTPS or literal loopback HTTP, without credentials, query or fragment",
        ));
    }
    Ok(if origin_only {
        url.origin().ascii_serialization()
    } else {
        url.to_string()
    })
}

fn digest(secret: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.as_bytes()).to_vec()
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn write_error(error: sqlx::Error) -> Error {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        Error::Conflict("identity is already registered")
    } else {
        Error::Database(error)
    }
}

pub use accounts::Credentials;
pub use registry::{GroupView, Topology};
#[cfg(test)]
#[path = "../tests/internal/mod.rs"]
mod tests;
