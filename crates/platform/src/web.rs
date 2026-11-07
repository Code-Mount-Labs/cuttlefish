//! JSON-first API used by the separate browser client and future mobile clients.
//! The platform serves only static HTML; it never renders account data into HTML.

use crate::{Credentials, Error, Platform};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use orchestrator_protocol::{
    ids::{GroupId, InstallationId},
    installation::{InstallationHeartbeat, Registration},
    sync::NodeToken,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

/// Build the stateless platform API. Account, session and installation state all
/// live in `PostgreSQL`; restarting a web replica never changes router ownership.
pub fn router(platform: Platform) -> Router {
    Router::new()
        .merge(crate::relay::web::routes())
        .route(
            "/",
            get(|| async { Html(include_str!("../../../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/app.js"),
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/app.css"),
                )
            }),
        )
        .route(
            "/logo.png",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    &include_bytes!("../../../assets/brand/cuttlefish-logo.png")[..],
                )
            }),
        )
        .route("/healthz", get(health))
        .route("/api/v1/status", get(status))
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/session", get(session))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/groups", get(groups).post(create_group))
        .route("/api/v1/groups/{group}/installations", get(installations))
        .route("/api/v1/groups/{group}/invitations", post(invite))
        .route("/api/v1/installations/preflight", post(preflight))
        .route("/api/v1/installations", post(enroll))
        .route("/api/v1/installations/self", get(installation))
        .route("/api/v1/installations/heartbeat", post(heartbeat))
        .route("/api/v1/installations/address", post(update_address))
        .route("/api/v1/installations/router-address", post(router_address))
        .route("/api/v1/router/authorize-agent", post(authorize_agent))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(axum::middleware::map_response(security_headers))
        .with_state(platform)
}

pub(crate) async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; font-src 'self' data:; img-src 'self' blob: data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn origin(platform: &Platform, headers: &HeaderMap, required: bool) -> Result<(), Error> {
    let origins: Vec<_> = headers.get_all(header::ORIGIN).iter().collect();
    if origins.is_empty() && !required {
        return Ok(());
    }
    if origins.len() != 1 || origins[0].to_str().ok() != Some(platform.origin.as_str()) {
        return Err(Error::Unauthorized);
    }
    Ok(())
}

pub(crate) fn token(headers: &HeaderMap) -> Result<NodeToken, Error> {
    let mut tokens = Vec::new();
    for value in headers.get_all(header::AUTHORIZATION) {
        tokens.push(
            value
                .to_str()
                .ok()
                .and_then(|value| value.strip_prefix("Bearer "))
                .ok_or(Error::Unauthorized)?,
        );
    }
    for value in headers.get_all(header::COOKIE) {
        for cookie in value.to_str().map_err(|_| Error::Unauthorized)?.split(';') {
            if let Some(value) = cookie.trim().strip_prefix("cuttlefish_session=") {
                tokens.push(value);
            }
        }
    }
    if tokens.len() != 1 {
        return Err(Error::Unauthorized);
    }
    NodeToken::try_from(tokens[0].to_owned()).map_err(|_| Error::Unauthorized)
}

fn mutation_token(platform: &Platform, headers: &HeaderMap) -> Result<NodeToken, Error> {
    origin(
        platform,
        headers,
        !headers.contains_key(header::AUTHORIZATION),
    )?;
    token(headers)
}

fn signed_in(platform: &Platform, token: &str, age: u32) -> Response {
    let secure = if platform.origin.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    ([(header::SET_COOKIE,format!("cuttlefish_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={age}{secure}"))],Json(json!({"token":token,"expires_in_seconds":age}))).into_response()
}

async fn health(State(platform): State<Platform>) -> Result<Json<serde_json::Value>, Error> {
    sqlx::query("SELECT 1").execute(&platform.pool).await?;
    Ok(Json(json!({"ok":true})))
}
async fn status(State(platform): State<Platform>) -> Json<serde_json::Value> {
    Json(json!({"service":"cuttlefish-platform","api_version":1,"origin":platform.origin}))
}
async fn register(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Response, Error> {
    origin(&platform, &headers, false)?;
    let token = platform.register_account(credentials).await?;
    Ok(signed_in(&platform, &token, 28_800))
}
async fn login(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(credentials): Json<Credentials>,
) -> Result<Response, Error> {
    origin(&platform, &headers, false)?;
    let token = platform.login(credentials).await?;
    Ok(signed_in(&platform, &token, 28_800))
}
async fn session(
    State(platform): State<Platform>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        json!({"account_id":platform.account(&token(&headers)?).await?}),
    ))
}
async fn logout(State(platform): State<Platform>, headers: HeaderMap) -> Result<Response, Error> {
    platform
        .logout(&mutation_token(&platform, &headers)?)
        .await?;
    Ok(signed_in(&platform, "", 0))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    after: Uuid,
}
async fn groups(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> Result<Json<Vec<crate::GroupView>>, Error> {
    Ok(Json(platform.groups(&token(&headers)?, page.after).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}
async fn create_group(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(input): Json<Name>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        json!({"group_id":platform.create_group(&mutation_token(&platform,&headers)?,&input.name).await?}),
    ))
}
async fn installations(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Path(group): Path<GroupId>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<orchestrator_protocol::installation::InstallationView>>, Error> {
    Ok(Json(
        platform
            .installations(&token(&headers)?, group, page.after)
            .await?,
    ))
}
async fn invite(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Path(group): Path<GroupId>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        json!({"invitation":platform.invite(&mutation_token(&platform,&headers)?,group).await?,"expires_in_seconds":1800}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preflight {
    group_id: GroupId,
    invitation: String,
}
async fn preflight(
    State(platform): State<Platform>,
    Json(input): Json<Preflight>,
) -> Result<Json<crate::Topology>, Error> {
    let invitation = NodeToken::try_from(input.invitation).map_err(|_| Error::Unauthorized)?;
    Ok(Json(platform.preflight(input.group_id, &invitation).await?))
}
async fn enroll(
    State(platform): State<Platform>,
    Json(input): Json<Registration>,
) -> Result<Json<serde_json::Value>, Error> {
    let router_url = platform.register_installation(&input).await?;
    Ok(Json(
        json!({"group_id":input.group_id,"installation_id":input.installation_id,"router_url":router_url}),
    ))
}
async fn installation(
    State(platform): State<Platform>,
    headers: HeaderMap,
) -> Result<Json<orchestrator_protocol::installation::InstallationView>, Error> {
    Ok(Json(platform.installation(&token(&headers)?).await?))
}
async fn heartbeat(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(input): Json<InstallationHeartbeat>,
) -> Result<Json<serde_json::Value>, Error> {
    platform.heartbeat(&token(&headers)?, &input).await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizeAgent {
    installation_id: InstallationId,
    credential: String,
}
async fn authorize_agent(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(input): Json<AuthorizeAgent>,
) -> Result<Json<serde_json::Value>, Error> {
    let credential = NodeToken::try_from(input.credential).map_err(|_| Error::Unauthorized)?;
    Ok(Json(
        json!({"group_id":platform.authorize_agent(&token(&headers)?,input.installation_id,&credential).await?}),
    ))
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Limited => StatusCode::TOO_MANY_REQUESTS,
            Self::Transport(_)
            | Self::Database(_)
            | Self::Migration(_)
            | Self::Worker(_)
            | Self::Password => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(json!({"error":self.to_string()}))).into_response()
    }
}

async fn update_address(
    State(platform): State<Platform>,
    headers: HeaderMap,
    Json(input): Json<orchestrator_protocol::remote::AddressUpdate>,
) -> Result<Json<serde_json::Value>, Error> {
    platform.update_address(&token(&headers)?, &input).await?;
    Ok(Json(json!({"ok":true})))
}
async fn router_address(
    State(platform): State<Platform>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        json!({"router_url":platform.router_address(&token(&headers)?).await?}),
    ))
}
