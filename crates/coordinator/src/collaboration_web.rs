//! Authenticated collaboration endpoints. Human approval has no agent HTTP route.
use crate::{Error, routing::RoutingService, store::Store, web::token};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post},
};
use orchestrator_protocol::{
    collaboration::{CapabilityRelease, GoalCommand, GoalSpec, GoalView},
    ids::{GoalId, TaskId},
    routing::DispatchView,
};
pub(crate) fn store_router() -> Router<Store> {
    Router::new()
        .route("/v1/goals", post(create).get(list))
        .route("/v1/goals/{goal}", get(read))
        .route("/v1/goals/{goal}/commands", post(command))
        .route("/v1/capabilities", get(catalog))
        .route("/v1/capabilities/installations", get(installations))
}

async fn installations(
    State(store): State<Store>,
    headers: HeaderMap,
) -> Result<Json<Vec<orchestrator_protocol::collaboration::InstalledCapability>>, Error> {
    Ok(Json(store.installations(&token(&headers)?).await?))
}
pub(crate) fn routing_router() -> Router<RoutingService> {
    Router::new().route("/v1/goals/{goal}/tasks/{task}/dispatch", post(dispatch))
}
async fn create(
    State(store): State<Store>,
    headers: HeaderMap,
    Json(spec): Json<GoalSpec>,
) -> Result<Json<GoalView>, Error> {
    Ok(Json(store.create_goal(&token(&headers)?, &spec).await?))
}
async fn list(
    State(store): State<Store>,
    headers: HeaderMap,
) -> Result<Json<Vec<GoalView>>, Error> {
    Ok(Json(store.goals(&token(&headers)?).await?))
}
async fn read(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(goal): Path<GoalId>,
) -> Result<Json<GoalView>, Error> {
    Ok(Json(store.goal(&token(&headers)?, goal).await?))
}
async fn command(
    State(store): State<Store>,
    headers: HeaderMap,
    Path(goal): Path<GoalId>,
    Json(command): Json<GoalCommand>,
) -> Result<Json<GoalView>, Error> {
    Ok(Json(
        store
            .goal_command(&token(&headers)?, goal, &command)
            .await?,
    ))
}
async fn catalog(
    State(store): State<Store>,
    headers: HeaderMap,
) -> Result<Json<Vec<CapabilityRelease>>, Error> {
    Ok(Json(store.catalog(&token(&headers)?).await?))
}
async fn dispatch(
    State(service): State<RoutingService>,
    headers: HeaderMap,
    Path((goal, task)): Path<(GoalId, TaskId)>,
) -> Result<Json<DispatchView>, Error> {
    Ok(Json(service.delegate(&token(&headers)?, goal, task).await?))
}
