//! REST handler for `/api/v1/inventory/resources/{entity_type}/bulk`.
//!
//! All SQL and business logic live behind `BulkResourceService`; this module
//! only parses requests, delegates, and shapes JSON responses.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::application::{
    BulkOperationInput, BulkOperationOutcome, BulkResultItem,
};
use crate::errors::AppError;

use super::super::{ensure_access, parse_required_string};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResourceBulkPayload {
    pub operation: String,
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<Value>,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResourceBulkResultItem {
    pub id: Option<String>,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResourceBulkResponse {
    pub entity_type: String,
    pub operation: String,
    pub total: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<ResourceBulkResultItem>,
}

fn map_item(i: BulkResultItem) -> ResourceBulkResultItem {
    ResourceBulkResultItem {
        id: i.id,
        success: i.success,
        error: i.error,
    }
}

fn map_outcome(o: BulkOperationOutcome) -> ResourceBulkResponse {
    ResourceBulkResponse {
        entity_type: o.entity_type,
        operation: o.operation,
        total: o.total,
        success_count: o.success_count,
        failure_count: o.failure_count,
        results: o.results.into_iter().map(map_item).collect(),
    }
}

fn action_for_bulk_operation(operation: &str) -> Result<Action, AppError> {
    match operation {
        "delete" => Ok(Action::Delete),
        "set_status" | "assign_tenant" | "set_role" | "set_site" | "set_cluster"
        | "set_device_type" => Ok(Action::Update),
        _ => Err(AppError::Validation(format!(
            "Unsupported bulk operation '{operation}'"
        ))),
    }
}

/// Apply a bulk operation across inventory resources.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/resources/{entity_type}/bulk",
    tag = "inventory",
    params(
        ("entity_type" = String, Path, description = "Inventory entity type"),
    ),
    request_body = ResourceBulkPayload,
    responses(
        (status = 200, description = "Resource bulk operation result", body = ResourceBulkResponse),
        (status = "4XX", description = "Resource bulk operation request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Resource bulk operation failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn execute_resource_bulk_operation(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(entity_type): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ResourceBulkPayload>,
) -> impl IntoResponse {
    let entity_type = match parse_required_string(&entity_type, "entity_type") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let operation = match parse_required_string(&payload.operation, "operation") {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let required_action = match action_for_bulk_operation(&operation) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        required_action,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    match state
        .services
        .inventory_bulk_resources
        .execute(BulkOperationInput {
            entity_type,
            operation,
            items: payload.items,
            params: payload.params,
        })
        .await
    {
        Ok(outcome) => (StatusCode::OK, Json(map_outcome(outcome))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::action_for_bulk_operation;
    use crate::contexts::access_control::domain::Action;

    #[test]
    fn bulk_operation_action_mapping_is_scope_specific() {
        assert_eq!(action_for_bulk_operation("delete").unwrap(), Action::Delete);
        for operation in [
            "set_status",
            "assign_tenant",
            "set_role",
            "set_site",
            "set_cluster",
            "set_device_type",
        ] {
            assert_eq!(
                action_for_bulk_operation(operation).unwrap(),
                Action::Update
            );
        }
        assert!(action_for_bulk_operation("unknown").is_err());
    }
}
