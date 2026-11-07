//! Small HTTP boundary. TLS is terminated at the deployment's reverse proxy.

use crate::{Error, store::Store};
use crate::{
    routing::RoutingService,
    selector::{Selector, Unconfigured},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use orchestrator_protocol::sync::{EventView, NodeToken, NodeView, SyncRequest, SyncResponse};
use orchestrator_protocol::{
    ids::ThreadId,
    routing::{DispatchBatch, DispatchPoll, DispatchView, RouteRequest},
};
use serde::Deserialize;
use std::sync::Arc;

/// Construct interchangeable coordinator replicas over a shared store.
pub fn router(store: Store) -> Router {
    router_with_selector(store, Arc::new(Unconfigured))
}

/// Construct a router with a configured model selection boundary.
pub fn router_with_selector(store: Store, selector: Arc<dyn Selector>) -> Router {
    let routing = Router::new()
        .route("/v1/tasks", post(submit))
        .route("/v1/threads/{thread}/failover", post(failover))
        .merge(crate::collaboration_web::routing_router())
        .with_state(RoutingService::new(store.clone(), selector));
    Router::new()
        .route("/v1/sync", post(sync))
        .route("/v1/nodes", get(nodes))
        .route(
            "/v1/attachments",
            post(upload_attachment).layer(DefaultBodyLimit::max(300_000)),
        )
        .route("/v1/attachments/{id}/chunks/{index}", get(attachment_chunk))
        .route("/v1/workspaces", get(workspaces).post(advertise_workspaces))
        .route("/v1/events", get(events))
        .route("/v1/threads/{thread}", get(thread))
        .route("/v1/dispatch/poll", post(poll))
        .merge(crate::collaboration_web::store_router())
        .with_state(store)
        .merge(routing)
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
}

async fn submit(
    State(service): State<RoutingService>,
    headers: HeaderMap,
    Json(request): Json<RouteRequest>,
) -> Result<Json<DispatchView>, Error> {
    Ok(Json(service.submit(&token(&headers)?, request).await?))
}

async fn failover(
    State(service): State<RoutingService>,
    headers: HeaderMap,
    Path(thread): Path<ThreadId>,
) -> Result<Json<DispatchView>, Error> {
    Ok(Json(service.failover(&token(&headers)?, thread).await?))
}

async fn thread(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(thread): Path<ThreadId>,
) -> Result<Json<DispatchView>, Error> {
    Ok(Json(store.thread(&token(&headers)?, thread).await?))
}

async fn poll(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(request): Json<DispatchPoll>,
) -> Result<Json<DispatchBatch>, Error> {
    Ok(Json(
        store.poll_dispatch(&token(&headers)?, &request).await?,
    ))
}

pub(crate) fn token(headers: &HeaderMap) -> Result<NodeToken, Error> {
    if headers.get_all(AUTHORIZATION).iter().count() != 1 {
        return Err(Error::Unauthorized);
    }
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(Error::Unauthorized)?;
    NodeToken::try_from(value.to_owned()).map_err(|_| Error::Unauthorized)
}

async fn sync(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(report): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, Error> {
    Ok(Json(store.sync(&token(&headers)?, &report).await?))
}

async fn nodes(
    State(store): State<Store>,
    headers: HeaderMap,
) -> Result<Json<Vec<NodeView>>, Error> {
    Ok(Json(store.nodes(&token(&headers)?).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(default)]
    after: i64,
}

async fn events(
    State(store): State<Store>,
    headers: HeaderMap,
    Query(cursor): Query<Cursor>,
) -> Result<Json<Vec<EventView>>, Error> {
    Ok(Json(store.events(&token(&headers)?, cursor.after).await?))
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "node authentication required"),
            Self::Conflict => (StatusCode::CONFLICT, "event identity conflict"),
            Self::RoutingConflict(_) | Self::Collaboration(_) => (
                StatusCode::CONFLICT,
                "routing request conflicts with saved state",
            ),
            Self::Unresolved => (
                StatusCode::CONFLICT,
                "execution may still be active; failover refused",
            ),
            Self::NoCapacity => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no eligible execution capacity",
            ),
            Self::Model => (StatusCode::BAD_GATEWAY, "model selection unavailable"),
            Self::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid synchronization request"),
            Self::Database(_) | Self::Migration(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "coordination storage unavailable",
            ),
        };
        (status, Json(serde_json::json!({"error":message}))).into_response()
    }
}

async fn workspaces(
    State(store): State<Store>,
    headers: HeaderMap,
) -> Result<Json<Vec<serde_json::Value>>, Error> {
    Ok(Json(store.provider_workspaces(&token(&headers)?).await?))
}
async fn advertise_workspaces(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(items): Json<Vec<orchestrator_protocol::execution::ProviderWorkspace>>,
) -> Result<Json<serde_json::Value>, Error> {
    store
        .advertise_workspaces(&token(&headers)?, &items)
        .await?;
    Ok(Json(serde_json::json!({"ok":true})))
}

async fn upload_attachment(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(chunk): Json<orchestrator_protocol::attachments::AttachmentChunk>,
) -> Result<Json<serde_json::Value>, Error> {
    let ready = store.put_attachment(&token(&headers)?, &chunk).await?;
    Ok(Json(serde_json::json!({"ready":ready})))
}
async fn attachment_chunk(
    State(store): State<Store>,
    headers: HeaderMap,
    Path((id, index)): Path<(orchestrator_protocol::ids::AttachmentId, u32)>,
) -> Result<Json<orchestrator_protocol::attachments::AttachmentChunk>, Error> {
    Ok(Json(
        store.attachment_chunk(&token(&headers)?, id, index).await?,
    ))
}
