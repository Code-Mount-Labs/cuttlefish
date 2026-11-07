//! Bounded JSON gateway. Untrusted installation HTML and cookies never share an origin.
use crate::{Error, Platform};
use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Request, header},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
};
use orchestrator_protocol::sync::NodeToken;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone)]
struct Gateway {
    platform: Platform,
    client: reqwest::Client,
    upstream: String,
    slots: Arc<tokio::sync::Semaphore>,
}
/// Create a public JSON gateway to a fixed loopback-only frps HTTP listener.
///
/// # Errors
/// Rejects non-loopback upstreams or HTTP client initialization failures.
pub fn router(platform: Platform, upstream: &str) -> Result<Router, Error> {
    with_builder(platform, upstream, reqwest::Client::builder())
}
fn with_builder(
    platform: Platform,
    upstream: &str,
    builder: reqwest::ClientBuilder,
) -> Result<Router, Error> {
    let url = reqwest::Url::parse(upstream).map_err(|_| Error::Invalid("invalid_frps_upstream"))?;
    if url.scheme() != "http"
        || !url
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback())
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Invalid("frps_upstream_must_be_loopback"));
    }
    let client = builder
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Transport("relay_client_unavailable"))?;
    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/i/{id}", get(index))
        .route("/i/{id}/", get(index))
        .route("/i/{id}/{*tail}", any(forward))
        .layer(axum::extract::DefaultBodyLimit::max(1_048_576))
        .layer(axum::middleware::map_response(crate::web::security_headers))
        .with_state(Gateway {
            platform,
            client,
            upstream: url.origin().ascii_serialization(),
            slots: Arc::new(tokio::sync::Semaphore::new(128)),
        }))
}
async fn health(State(gateway): State<Gateway>) -> Result<Json<Value>, Error> {
    sqlx::query("SELECT 1")
        .execute(&gateway.platform.pool)
        .await?;
    Ok(Json(
        serde_json::json!({"ok":true,"service":"cuttlefish-relay"}),
    ))
}
async fn index(
    State(gateway): State<Gateway>,
    Path(id): Path<Uuid>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, Error> {
    gateway.platform.relay_epoch(id).await?;
    if !uri.path().ends_with('/') {
        return Ok(axum::response::Redirect::temporary(&format!("/i/{id}/")).into_response());
    }
    Ok(Html(include_str!("../../../../web/local/index.html")).into_response())
}
async fn forward(
    State(gateway): State<Gateway>,
    Path((id, tail)): Path<(Uuid, String)>,
    request: Request<Body>,
) -> Result<Response, Error> {
    let epoch = gateway.platform.relay_epoch(id).await?;
    let _permit = gateway
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Limited)?;
    if request.method() == "GET" {
        match tail.as_str() {
            "app.js" => {
                return Ok((
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../../../../web/local/app.js"),
                )
                    .into_response());
            }
            "app.css" => {
                return Ok((
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../../../../web/local/app.css"),
                )
                    .into_response());
            }
            _ => {}
        }
    }
    validate_request(&request, &tail, &gateway.platform.relay_keys()?.relay)?;
    let path = format!(
        "/{}{}",
        tail,
        request
            .uri()
            .query()
            .map_or_else(String::new, |q| format!("?{q}"))
    );
    let mut outgoing = gateway
        .client
        .request(
            request.method().clone(),
            format!("{}{path}", gateway.upstream),
        )
        .header(header::HOST, format!("{id}.{epoch}.cuttlefish.invalid"));
    for name in [header::AUTHORIZATION, header::CONTENT_TYPE, header::ORIGIN] {
        let values: Vec<_> = request.headers().get_all(&name).iter().collect();
        if values.len() > 1 {
            return Err(Error::Unauthorized);
        }
        if let Some(value) = values.first() {
            outgoing = outgoing.header(name, (*value).clone());
        }
    }
    if let Some(cookie) = session_cookie(request.headers())? {
        outgoing = outgoing.header(header::COOKIE, cookie);
    }
    let body = axum::body::to_bytes(request.into_body(), 1_048_576)
        .await
        .map_err(|_| Error::Invalid("relay_request_too_large"))?;
    let response = outgoing
        .body(body)
        .send()
        .await
        .map_err(|_| Error::Transport("relay_connection_unavailable"))?;
    json_response(response, id).await
}
fn validate_request(request: &Request<Body>, tail: &str, origin: &str) -> Result<(), Error> {
    let raw = request.uri().path();
    if !matches!(request.method().as_str(), "GET" | "POST")
        || raw.contains(['%', '\\'])
        || raw.contains("//")
        || tail.split('/').any(|p| p == "." || p == "..")
        || !(tail.starts_with("api/local/") || tail == "api/node/enroll" || tail.starts_with("v1/"))
        || tail == "api/local/setup"
    {
        return Err(Error::Invalid("relay_route_not_allowed"));
    }
    if let Some(value) = request.headers().get(header::ORIGIN)
        && value.to_str().ok() != Some(origin)
    {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
fn session_cookie(headers: &HeaderMap) -> Result<Option<String>, Error> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for part in header.to_str().map_err(|_| Error::Unauthorized)?.split(';') {
            if let Some(value) = part.trim().strip_prefix("cuttlefish_local_session=") {
                NodeToken::try_from(value.to_owned()).map_err(|_| Error::Unauthorized)?;
                if found.is_some() {
                    return Err(Error::Unauthorized);
                }
                found = Some(format!("cuttlefish_local_session={value}"));
            }
        }
    }
    Ok(found)
}
async fn json_response(mut response: reqwest::Response, id: Uuid) -> Result<Response, Error> {
    let status = response.status();
    if status.is_redirection()
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            != Some("application/json")
    {
        return Err(Error::Transport("relay_expected_json_response"));
    }
    let cookie = response_cookie(response.headers(), id)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Transport("relay_response_interrupted"))?
    {
        if bytes.len() + chunk.len() > 1_048_576 {
            return Err(Error::Transport("relay_response_too_large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Transport("relay_invalid_json_response"))?;
    let response = (status, Json(value)).into_response();
    Ok(if let Some(cookie) = cookie {
        ([(header::SET_COOKIE, cookie)], response).into_response()
    } else {
        response
    })
}
fn response_cookie(headers: &HeaderMap, id: Uuid) -> Result<Option<String>, Error> {
    let values: Vec<_> = headers.get_all(header::SET_COOKIE).iter().collect();
    if values.is_empty() {
        return Ok(None);
    }
    if values.len() != 1 {
        return Err(Error::Transport("relay_invalid_session_cookie"));
    }
    let text = values[0]
        .to_str()
        .map_err(|_| Error::Transport("relay_invalid_session_cookie"))?;
    let mut parts = text.split(';');
    let value = parts
        .next()
        .and_then(|v| v.strip_prefix("cuttlefish_local_session="))
        .ok_or(Error::Transport("relay_invalid_session_cookie"))?;
    if !value.is_empty() {
        NodeToken::try_from(value.to_owned())
            .map_err(|_| Error::Transport("relay_invalid_session_cookie"))?;
    }
    let age = parts
        .find_map(|v| v.trim().strip_prefix("Max-Age="))
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v <= 28_800)
        .ok_or(Error::Transport("relay_invalid_session_cookie"))?;
    Ok(Some(format!(
        "cuttlefish_local_session={value}; HttpOnly; Secure; SameSite=Strict; Path=/i/{id}/; Max-Age={age}"
    )))
}

#[cfg(test)]
#[path = "../../tests/internal/relay_gateway.rs"]
mod tests;
