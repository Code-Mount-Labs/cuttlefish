//! Private frps authorization hook. frps never selects a tenant route by itself.
use crate::{Error, Platform};
use serde_json::{Value, json};
use uuid::Uuid;

impl Platform {
    /// Authorize a frps hook and replace client-selected routing fields with canonical values.
    /// This endpoint must listen on loopback only and must not be reverse-proxied publicly.
    ///
    /// # Errors
    /// Rejects forged identities, stale epochs, unsupported operations and storage failures.
    pub async fn relay_operation(&self, operation: &str, content: &Value) -> Result<Value, Error> {
        let keys = self.relay_keys()?;
        match operation {
            "Login" => {
                let claims = keys.verify(string(content, "privilege_key")?)?;
                if string(content, "user")? != claims.sub.to_string()
                    || self.relay_epoch(claims.sub).await? != claims.epoch
                {
                    return Err(Error::Unauthorized);
                }
                let mut canonical = content.clone();
                canonical["run_id"] = json!(run_id(claims.sub, claims.epoch));
                canonical["metas"] = json!({"epoch":claims.epoch});
                Ok(json!({"unchange":false,"content":canonical}))
            }
            "NewProxy" | "Ping" | "NewWorkConn" => {
                let user = &content["user"];
                let id = Uuid::parse_str(string(user, "user")?).map_err(|_| Error::Unauthorized)?;
                let epoch = self.relay_epoch(id).await?;
                if user["metas"]["epoch"] != epoch.to_string()
                    || user["run_id"] != run_id(id, epoch)
                {
                    return Err(Error::Unauthorized);
                }
                if operation == "NewProxy" {
                    return proxy(content, id, epoch);
                }
                let claims = keys.verify(string(content, "privilege_key")?)?;
                if claims.sub != id || claims.epoch != epoch {
                    return Err(Error::Unauthorized);
                }
                Ok(json!({"unchange":true}))
            }
            _ => Err(Error::Unauthorized),
        }
    }
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key].as_str().ok_or(Error::Unauthorized)
}
fn run_id(id: Uuid, epoch: Uuid) -> String {
    format!("{}{}", id.simple(), epoch.simple())
}
fn proxy(content: &Value, id: Uuid, epoch: Uuid) -> Result<Value, Error> {
    let name = format!("{id}.{epoch}.dashboard");
    let domains = json!([format!("{id}.{epoch}.cuttlefish.invalid")]);
    if content["proxy_name"] != name
        || content["proxy_type"] != "http"
        || content["custom_domains"] != domains
        || content["locations"] != json!(["/"])
    {
        return Err(Error::Unauthorized);
    }
    // Construct a fresh allowlisted object. In particular, no client-controlled headers,
    // response headers, alternate ports, load-balancing groups or additional domains survive.
    Ok(
        json!({"unchange":false,"content":{"user":content["user"],"proxy_name":name,
        "proxy_type":"http","custom_domains":domains,"locations":["/"],
        "bandwidth_limit":"1MB","bandwidth_limit_mode":"server"}}),
    )
}
