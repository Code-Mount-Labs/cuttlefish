//! Signing and verification boundary for short-lived connector credentials.
use crate::{Error, validate_url};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// Configured identity issuer and relay audience. The private key is optional on relays.
pub struct Keys {
    pub(super) issuer: String,
    pub(crate) relay: String,
    public: String,
    decoding: DecodingKey,
    encoding: Option<EncodingKey>,
}
impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys([signing material redacted])")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Claims {
    pub sub: Uuid,
    pub epoch: Uuid,
    pub iss: String,
    pub aud: String,
    pub iat: u64,
    pub exp: u64,
}
impl Keys {
    /// Configure an Ed25519 public JWK component and optional PKCS#8 PEM signer.
    ///
    /// # Errors
    /// Rejects invalid URLs, key material and mismatched signing/verification keys.
    pub fn new(
        issuer: &str,
        relay: &str,
        public: &str,
        private: Option<&str>,
    ) -> Result<Self, Error> {
        let issuer = validate_url(issuer, true)?;
        let relay = validate_url(relay, true)?;
        let decoding = DecodingKey::from_ed_components(public)
            .map_err(|_| Error::Invalid("invalid_relay_public_key"))?;
        let encoding = private
            .map(|value| {
                EncodingKey::from_ed_pem(value.as_bytes())
                    .map_err(|_| Error::Invalid("invalid_relay_private_key"))
            })
            .transpose()?;
        let keys = Self {
            issuer,
            relay,
            public: public.into(),
            decoding,
            encoding,
        };
        if keys.encoding.is_some() {
            let token = keys.issue(
                Uuid::new_v4(),
                Uuid::new_v4(),
                jsonwebtoken::get_current_timestamp(),
            )?;
            keys.verify(&token)?;
        }
        Ok(keys)
    }
    pub(super) fn issue(&self, sub: Uuid, epoch: Uuid, now: u64) -> Result<String, Error> {
        let key = self
            .encoding
            .as_ref()
            .ok_or(Error::Invalid("relay_signer_not_configured"))?;
        let claims = Claims {
            sub,
            epoch,
            iss: self.issuer.clone(),
            aud: self.relay.clone(),
            iat: now,
            exp: now
                .checked_add(300)
                .ok_or(Error::Invalid("invalid_token_time"))?,
        };
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("cuttlefish-relay-1".into());
        jsonwebtoken::encode(&header, &claims, key)
            .map_err(|_| Error::Invalid("relay_signing_failed"))
    }
    pub(super) fn verify(&self, token: &str) -> Result<Claims, Error> {
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.relay]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
        validation.leeway = 0;
        let claims = jsonwebtoken::decode::<Claims>(token, &self.decoding, &validation)
            .map_err(|_| Error::Unauthorized)?
            .claims;
        if claims.sub.is_nil()
            || claims.epoch.is_nil()
            || claims.iat > jsonwebtoken::get_current_timestamp()
        {
            return Err(Error::Unauthorized);
        }
        Ok(claims)
    }
    pub(super) fn jwks(&self) -> Value {
        json!({"keys":[{"kty":"OKP","crv":"Ed25519","alg":"EdDSA","use":"sig","kid":"cuttlefish-relay-1","x":self.public}]})
    }
    pub(super) fn discovery(&self) -> Value {
        json!({"issuer":self.issuer,"jwks_uri":format!("{}/api/v1/relay/keys",self.issuer),"token_endpoint":format!("{}/api/v1/relay/token",self.issuer),"id_token_signing_alg_values_supported":["EdDSA"],"response_types_supported":["token"],"subject_types_supported":["public"]})
    }
}

#[cfg(test)]
#[path = "../../tests/internal/relay_keys.rs"]
mod tests;
