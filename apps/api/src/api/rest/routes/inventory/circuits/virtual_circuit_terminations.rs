use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::circuits::virtual_circuit_termination::{CONTENT_TYPE, VirtualCircuitTermination};
use crate::contexts::inventory::infrastructure::persistence::repositories::circuits::virtual_circuit_termination_repository::{VIRTUAL_CIRCUIT_TERMINATION_COLUMNS, VirtualCircuitTerminationChanges, VirtualCircuitTerminationFilter};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use super::super::{InventoryListEnvelope, masked_custom_fields, masked_custom_fields_page, parse_uuid, validate_and_split_custom_fields};

fn default_role() -> String {
    "peer".into()
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualCircuitTerminationCreatePayload {
    pub virtual_circuit_id: Uuid,
    pub interface_id: Uuid,
    #[serde(default = "default_role")]
    #[schema(schema_with = crate::contexts::inventory::domain::circuits::virtual_circuit_termination::role_schema)]
    pub role: String,
    #[serde(default)]
    #[schema(max_length = 200)]
    pub description: String,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VirtualCircuitTerminationPatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub virtual_circuit_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub interface_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with = crate::contexts::inventory::domain::circuits::virtual_circuit_termination::role_schema)]
    pub role: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct VirtualCircuitTerminationListQuery {
    pub search: Option<String>,
    pub virtual_circuit_id: Option<String>,
    pub interface_id: Option<String>,
    pub role: Option<String>,
    /// Exact keys: virtual_circuit_id, interface_id, role, description, created, last_updated.
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Evaluate persisted reference attributes supplied by the graph-held transaction.
async fn authorize_attributes(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Uuid,
    attributes: serde_json::Value,
) -> Result<(), AppError> {
    let attributes = serde_json::from_value(attributes)?;
    state
        .services
        .policy_evaluation
        .ensure_access_with_attributes(
            context,
            action,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
            Some(attributes),
        )
        .await
}

async fn mask(
    state: &AppState,
    mut item: VirtualCircuitTermination,
) -> Result<VirtualCircuitTermination, AppError> {
    item.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, item.id, &item.custom_fields).await?;
    Ok(item)
}

#[utoipa::path(get, path="/api/v1/inventory/virtual-circuit-terminations", tag="inventory", params(VirtualCircuitTerminationListQuery), responses((status=200, description="Virtual circuit terminations", body=InventoryListEnvelope<VirtualCircuitTermination>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_virtual_circuit_terminations(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VirtualCircuitTerminationListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Read).await?;
        let filter = VirtualCircuitTerminationFilter {
            search: search_pattern(query.search.as_deref()),
            virtual_circuit_id: parse_csv_uuids(
                query.virtual_circuit_id.as_deref(),
                "virtual_circuit_id",
            )?,
            interface_id: parse_csv_uuids(query.interface_id.as_deref(), "interface_id")?,
            role: parse_csv_strings(query.role.as_deref()),
            sort: SortSpec::parse_columns(
                query.sort.as_deref(),
                query.order.as_deref(),
                VIRTUAL_CIRCUIT_TERMINATION_COLUMNS,
            )?,
        };
        let (mut items, total) = state
            .repositories
            .virtual_circuit_terminations
            .list_paginated(query.limit, query.offset, filter)
            .await?;
        let fields = masked_custom_fields_page(
            &state,
            CONTENT_TYPE,
            items
                .iter()
                .map(|item| (item.id, item.custom_fields.clone()))
                .collect(),
        )
        .await?;
        for (item, fields) in items.iter_mut().zip(fields) {
            item.custom_fields = fields;
        }
        Ok::<_, AppError>(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(query.limit),
            offset: clamp_offset(query.offset),
        })
    }
    .await;
    match result {
        Ok(page) => Json(page).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(get, path="/api/v1/inventory/virtual-circuit-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="Virtual circuit termination", body=VirtualCircuitTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_virtual_circuit_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        let id = parse_uuid(&id, "virtual_circuit_termination_id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        let item = state
            .repositories
            .virtual_circuit_terminations
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Virtual circuit termination not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path="/api/v1/inventory/virtual-circuit-terminations", tag="inventory", request_body=VirtualCircuitTerminationCreatePayload, responses((status=201, description="Created", body=VirtualCircuitTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_virtual_circuit_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitTerminationCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Create).await?;
        let mut item = VirtualCircuitTermination {
            id: Uuid::now_v7(),
            virtual_circuit_id: payload.virtual_circuit_id,
            interface_id: payload.interface_id,
            role: payload.role,
            description: payload.description,
            tags: payload.tags.unwrap_or_default(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        item.validate()?;
        let outcome = validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE,
            payload.custom_fields.map(serde_json::Value::Object),
            true,
        )
        .await?;
        let intents = if let Some(outcome) = outcome {
            item.custom_fields = outcome.custom_field_data;
            outcome.secret_intents
        } else {
            Vec::new()
        };
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(item.id, intents)?;
        let item = state
            .repositories
            .virtual_circuit_terminations
            .create_with_secret_intents(&item, inputs, |action, id, attributes| {
                authorize_attributes(&state, &context, action, id, attributes)
            })
            .await?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(patch, path="/api/v1/inventory/virtual-circuit-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=VirtualCircuitTerminationPatchPayload, responses((status=200, description="Updated", body=VirtualCircuitTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_virtual_circuit_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VirtualCircuitTerminationPatchPayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "virtual_circuit_termination_id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        let mut changes = VirtualCircuitTerminationChanges {
            virtual_circuit_id: payload.virtual_circuit_id,
            interface_id: payload.interface_id,
            role: payload.role,
            description: payload.description,
            tags: payload.tags,
            custom_fields: None,
        };
        changes.validate()?;
        let outcome = validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE,
            payload.custom_fields.map(serde_json::Value::Object),
            false,
        )
        .await?;
        let intents = if let Some(outcome) = outcome {
            changes.custom_fields = Some(outcome.custom_field_data);
            outcome.secret_intents
        } else {
            Vec::new()
        };
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(id, intents)?;
        let item = state
            .repositories
            .virtual_circuit_terminations
            .update_with_secret_intents(id, &changes, inputs, |action, id, attributes| {
                authorize_attributes(&state, &context, action, id, attributes)
            })
            .await?
            .ok_or_else(|| AppError::NotFound("Virtual circuit termination not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path="/api/v1/inventory/virtual-circuit-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_virtual_circuit_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "virtual_circuit_termination_id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state
            .repositories
            .virtual_circuit_terminations
            .delete(id)
            .await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            handle_app_error(project_inventory_delete_error(&state, &context, error).await)
        }
    }
}
