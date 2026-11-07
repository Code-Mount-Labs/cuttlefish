//! Hosted token endpoints and the private frps hook.
use crate::{Error, Platform};
use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, header},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use orchestrator_protocol::{
    ids::InstallationId,
    remote::{RelayLease, RelayMutation},
    sync::NodeToken,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn routes() -> Router<Platform> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/api/v1/relay/keys", get(keys))
        .route("/api/v1/relay/token", post(token))
        .route("/api/v1/installations/relay", post(change))
}
async fn discovery(State(platform): State<Platform>) -> Result<Json<Value>, Error> {
    Ok(Json(platform.relay_keys()?.discovery()))
}
async fn keys(State(platform): State<Platform>) -> Result<Json<Value>, Error> {
    Ok(Json(platform.relay_keys()?.jwks()))
}
async fn change(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(input): Json<RelayMutation>,
) -> Result<Json<RelayLease>, Error> {
    Ok(Json(
        platform
            .change_relay(&crate::web::token(&headers)?, &input)
            .await?,
    ))
}
#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
}
async fn token(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Result<Json<Value>, Error> {
    if form.grant_type != "client_credentials" {
        return Err(Error::Unauthorized);
    }
    let (id, secret) = client_credentials(&headers)?;
    let token = platform.relay_token(id, &secret).await?;
    Ok(Json(
        json!({"access_token":token,"token_type":"Bearer","expires_in":300}),
    ))
}
fn client_credentials(headers: &HeaderMap) -> Result<(InstallationId, NodeToken), Error> {
    let values: Vec<_> = headers.get_all(header::AUTHORIZATION).iter().collect();
    if values.len() != 1 {
        return Err(Error::Unauthorized);
    }
    let encoded = values[0]
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix("Basic "))
        .ok_or(Error::Unauthorized)?;
    let bytes = STANDARD.decode(encoded).map_err(|_| Error::Unauthorized)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Unauthorized)?;
    let (id, secret) = text.split_once(':').ok_or(Error::Unauthorized)?;
    Ok((
        id.parse().map_err(|_| Error::Unauthorized)?,
        NodeToken::try_from(secret.to_owned()).map_err(|_| Error::Unauthorized)?,
    ))
}
/// Private loopback-only hook router for frps. Never mount it on the public gateway.
pub fn plugin_router(platform: Platform) -> Router {
    Router::new()
        .route("/frp", post(plugin))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
        .with_state(platform)
}
#[derive(Deserialize)]
struct Operation {
    op: String,
    version: String,
}
#[derive(Deserialize)]
struct Envelope {
    content: Value,
}
async fn plugin(
    State(platform): State<Platform>,
    Query(op): Query<Operation>,
    Json(input): Json<Envelope>,
) -> Json<Value> {
    let result = if op.version == "0.1.0" {
        platform.relay_operation(&op.op, &input.content).await
    } else {
        Err(Error::Unauthorized)
    };
    Json(
        result.unwrap_or_else(
            |_| json!({"reject":true,"reject_reason":"relay_authorization_rejected"}),
        ),
    )
}
