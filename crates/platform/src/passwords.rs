//! Blocking password work behind a small executor boundary. The account store
//! owns admission limits; a permit stays held until the worker actually exits.
use crate::{Credentials, Error};
use tokio::{sync::OwnedSemaphorePermit, task::JoinHandle};

type HashOutput = Result<(String, String), Error>;
type HashWorker = fn(HashJob) -> JoinHandle<HashOutput>;
type HashJob = Box<dyn FnOnce() -> Result<(String, String), Error> + Send>;
type VerifyJob = Box<dyn FnOnce() -> bool + Send>;
#[derive(Debug, Clone)]
pub(crate) struct Passwords {
    pub hash_worker: HashWorker,
    pub verify_worker: fn(VerifyJob) -> JoinHandle<bool>,
    pub hash_password: fn(&[u8]) -> Result<String, Error>,
}
impl Default for Passwords {
    fn default() -> Self {
        Self {
            hash_worker: tokio::task::spawn_blocking,
            verify_worker: tokio::task::spawn_blocking,
            hash_password: hash,
        }
    }
}
impl Passwords {
    pub async fn hash(
        &self,
        input: Credentials,
        permit: OwnedSemaphorePermit,
    ) -> Result<(String, String), Error> {
        let hasher = self.hash_password;
        (self.hash_worker)(Box::new(move || {
            let _permit = permit;
            hasher(input.password.as_bytes()).map(|hash| (input.username, hash))
        }))
        .await?
    }
    pub async fn verify(
        &self,
        password: String,
        hash: String,
        permit: OwnedSemaphorePermit,
    ) -> Result<bool, Error> {
        Ok((self.verify_worker)(Box::new(move || {
            use argon2::password_hash::{PasswordVerifier, phc::PasswordHash};
            let _permit = permit;
            PasswordHash::new(&hash).is_ok_and(|hash| {
                argon2::Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            })
        }))
        .await?)
    }
}
fn hash(password: &[u8]) -> Result<String, Error> {
    use argon2::password_hash::PasswordHasher;
    finish_hash(argon2::Argon2::default().hash_password(password))
}
fn finish_hash(
    result: Result<argon2::password_hash::phc::PasswordHash, argon2::password_hash::Error>,
) -> Result<String, Error> {
    result
        .map(|hash| hash.to_string())
        .map_err(|_| Error::Password)
}
#[cfg(test)]
#[path = "../tests/internal/passwords.rs"]
mod tests;
