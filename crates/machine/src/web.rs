//! JSON-first local setup dashboard, available before the execution database.
use crate::{
    Error, auth::Credentials, resources::DatabaseChoice, settings::ModelSettings, state::Machine,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use orchestrator_protocol::sync::NodeToken;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

/// Bind the authenticated local dashboard and independently authenticated router API.
pub fn router(machine: Machine) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../web/local/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../../../web/local/app.js"),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../../../web/local/app.css"),
                )
            }),
        )
        .route("/api/local/status", get(status))
        .route("/api/local/remote", get(remote_status).post(remote_select))
        .route("/api/local/relay-proof", post(remote_proof))
        .route("/api/node/enroll", post(enroll))
        .route("/api/local/setup", post(setup))
        .route("/api/local/login", post(login))
        .route("/api/local/logout", post(logout))
        .route("/api/local/config", get(configuration))
        .route("/api/local/database", post(database))
        .route("/api/local/database/retry", post(retry_database))
        .route("/api/local/model", post(model))
        .fallback(execution)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(axum::middleware::map_response(headers))
        .with_state(machine)
}
async fn headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
async fn origin(machine: &Machine, headers: &HeaderMap) -> Result<(), Error> {
    let public = machine.identity().await?.public_origin;
    let values: Vec<_> = headers.get_all(header::ORIGIN).iter().collect();
    let remote = machine.remote_origin().await?;
    if values.len() != 1
        || !(values[0].to_str().ok() == Some(public.as_str())
            || remote
                .as_deref()
                .is_some_and(|remote| values[0].to_str().ok() == Some(remote)))
    {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
fn token(headers: &HeaderMap) -> Result<NodeToken, Error> {
    let mut values = Vec::new();
    for value in headers.get_all(header::COOKIE) {
        for cookie in value.to_str().map_err(|_| Error::Unauthorized)?.split(';') {
            if let Some(value) = cookie.trim().strip_prefix("cuttlefish_local_session=") {
                values.push(value);
            }
        }
    }
    if values.len() != 1 {
        return Err(Error::Unauthorized);
    }
    NodeToken::try_from(values[0].to_owned()).map_err(|_| Error::Unauthorized)
}
async fn authorize(
    machine: &Machine,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<NodeToken, Error> {
    if mutation {
        origin(machine, headers).await?;
    }
    let token = token(headers)?;
    machine.authenticate(&token).await?;
    Ok(token)
}
async fn cookie(machine: &Machine, value: &str, age: u32) -> Result<Response, Error> {
    let identity = machine.identity().await?;
    let secure = if identity.public_origin.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    Ok(([(header::SET_COOKIE,format!("cuttlefish_local_session={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={age}{secure}"))],Json(json!({"ok":true}))).into_response())
}
async fn status(State(machine): State<Machine>) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(json!({"initialized":machine.initialized().await?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Setup {
    token: String,
    username: String,
    password: String,
}
async fn setup(
    State(machine): State<Machine>,
    headers: HeaderMap,
    Json(input): Json<Setup>,
) -> Result<Response, Error> {
    origin(&machine, &headers).await?;
    let token = NodeToken::try_from(input.token).map_err(|_| Error::Unauthorized)?;
    let session = machine
        .setup_admin(
            &token,
            Credentials {
                username: input.username,
                password: input.password,
            },
        )
        .await?;
    cookie(&machine, &session, 28_800).await
}
async fn login(
    State(machine): State<Machine>,
    headers: HeaderMap,
    Json(input): Json<Credentials>,
) -> Result<Response, Error> {
    origin(&machine, &headers).await?;
    let token = machine.login(input).await?;
    cookie(&machine, &token, 28_800).await
}
async fn logout(State(machine): State<Machine>, headers: HeaderMap) -> Result<Response, Error> {
    let token = authorize(&machine, &headers, true).await?;
    machine.logout(&token).await?;
    cookie(&machine, "", 0).await
}
async fn configuration(
    State(machine): State<Machine>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    authorize(&machine, &headers, false).await?;
    let identity = machine.identity().await?;
    Ok(Json(
        json!({"group_id":identity.registration.group_id,"installation_id":identity.registration.installation_id,"name":identity.registration.name,"role":identity.registration.role,"database":machine.status().await?,"model":machine.model_status().await?,"remote":machine.remote_status().await?}),
    ))
}
async fn database(
    State(machine): State<Machine>,
    headers: HeaderMap,
    Json(input): Json<DatabaseChoice>,
) -> Result<(StatusCode, Json<crate::resources::ResourceStatus>), Error> {
    authorize(&machine, &headers, true).await?;
    require_router(&machine).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(machine.select_database(input).await?),
    ))
}
async fn retry_database(
    State(machine): State<Machine>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<crate::resources::ResourceStatus>), Error> {
    authorize(&machine, &headers, true).await?;
    require_router(&machine).await?;
    Ok((StatusCode::ACCEPTED, Json(machine.retry_database().await?)))
}
async fn model(
    State(machine): State<Machine>,
    headers: HeaderMap,
    Json(input): Json<ModelSettings>,
) -> Result<Json<serde_json::Value>, Error> {
    authorize(&machine, &headers, true).await?;
    require_router(&machine).await?;
    machine.configure_model(&input).await?;
    Ok(Json(json!({"ok":true})))
}
async fn require_router(machine: &Machine) -> Result<(), Error> {
    if !machine.identity().await?.registration.role.has_router() {
        return Err(Error::Invalid("this_installation_has_no_router"));
    }
    Ok(())
}
async fn execution(
    State(machine): State<Machine>,
    request: Request<Body>,
) -> Result<Response, Error> {
    if !request.uri().path().starts_with("/v1/") {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let store = machine.execution_store().await?;
    let response = orchestrator_coordinator::web::router_with_selector(store, Arc::new(machine))
        .oneshot(request)
        .await;
    match response {
        Ok(response) => Ok(response),
        Err(never) => match never {},
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Limited => StatusCode::TOO_MANY_REQUESTS,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(json!({"error":self.to_string()}))).into_response()
    }
}

async fn enroll(
    State(machine): State<Machine>,
    Json(input): Json<crate::registry::Enrollment>,
) -> Result<Json<serde_json::Value>, Error> {
    let credential = NodeToken::try_from(input.credential).map_err(|_| Error::Unauthorized)?;
    let own = machine.identity().await?;
    let node = orchestrator_protocol::ids::NodeId::from(input.installation_id);
    let store = machine.execution_store().await?;
    if store
        .identity(&credential)
        .await
        .is_ok_and(|identity| identity == (own.registration.group_id, node))
    {
        return Ok(Json(json!({"ok":true})));
    }
    let group = machine
        .authorize_agent(input.installation_id, &credential)
        .await?;
    if store.enroll(group, node, &credential).await.is_err()
        && !store
            .identity(&credential)
            .await
            .is_ok_and(|identity| identity == (group, node))
    {
        return Err(Error::Dependency("agent_enrollment_failed"));
    }
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
#[path = "../tests/internal/web_faults.rs"]
mod tests;

async fn remote_status(
    State(machine): State<Machine>,
    headers: HeaderMap,
) -> Result<Json<crate::remote::RemoteStatus>, Error> {
    authorize(&machine, &headers, false).await?;
    Ok(Json(machine.remote_status().await?))
}
async fn remote_select(
    State(machine): State<Machine>,
    headers: HeaderMap,
    Json(choice): Json<orchestrator_protocol::remote::RemoteChoice>,
) -> Result<(StatusCode, Json<crate::remote::RemoteStatus>), Error> {
    authorize(&machine, &headers, true).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(machine.select_remote(choice).await?),
    ))
}
async fn remote_proof(State(machine): State<Machine>) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(machine.remote_proof().await?))
}
