use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use uuid::Uuid;

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::ensure_inventory_collection_access;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::object_ref::ObjectRefInput;
use crate::contexts::inventory::domain::vpn::l2vpn_termination::{CONTENT_TYPE, L2VpnTermination, validate_endpoint};
use crate::contexts::inventory::infrastructure::persistence::repositories::vpn::l2vpn_repository::{L2VPN_TERMINATION_COLUMNS, L2VpnTerminationChanges, L2VpnTerminationFilter, NewL2VpnTermination};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, masked_custom_fields, masked_custom_fields_page, parse_uuid,
    validate_and_split_custom_fields,
};
use super::l2vpns::{authorize_attributes, authorize_model};

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct L2VpnTerminationCreatePayload {
    pub l2vpn_id: Uuid,
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn_termination::endpoint_schema)]
    pub assigned_object: ObjectRefInput,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct L2VpnTerminationPatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub l2vpn_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn_termination::endpoint_schema)]
    pub assigned_object: Option<ObjectRefInput>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct L2VpnTerminationListQuery {
    pub search: Option<String>,
    pub l2vpn_id: Option<String>,
    pub assigned_object_type: Option<String>,
    pub assigned_object_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

async fn mask(state: &AppState, mut item: L2VpnTermination) -> Result<L2VpnTermination, AppError> {
    item.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, item.id, &item.custom_fields).await?;
    Ok(item)
}

#[utoipa::path(get, path="/api/v1/inventory/l2vpn-terminations", tag="inventory", params(L2VpnTerminationListQuery), responses((status=200, description="L2VPN terminations", body=InventoryListEnvelope<L2VpnTermination>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_l2vpn_terminations(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<L2VpnTerminationListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Read).await?;
        let filter = L2VpnTerminationFilter {
            search: search_pattern(query.search.as_deref()),
            l2vpn_id: parse_csv_uuids(query.l2vpn_id.as_deref(), "l2vpn_id")?,
            assigned_object_type: parse_csv_strings(query.assigned_object_type.as_deref()),
            assigned_object_id: parse_csv_uuids(
                query.assigned_object_id.as_deref(),
                "assigned_object_id",
            )?,
            sort: SortSpec::parse_columns(
                query.sort.as_deref(),
                query.order.as_deref(),
                L2VPN_TERMINATION_COLUMNS,
            )?,
        };
        let (mut items, total) = state
            .repositories
            .l2vpn_terminations
            .list_paginated(query.limit, query.offset, filter)
            .await?;
        let fields = masked_custom_fields_page(
            &state,
            CONTENT_TYPE,
            items
                .iter()
                .map(|i| (i.id, i.custom_fields.clone()))
                .collect(),
        )
        .await?;
        if fields.len() != items.len() {
            return Err(AppError::Internal(
                "custom-field projection count mismatch".into(),
            ));
        }
        for (item, value) in items.iter_mut().zip(fields) {
            item.custom_fields = value;
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
        Ok(items) => Json(items).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(get, path="/api/v1/inventory/l2vpn-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="L2VPN termination", body=L2VpnTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_l2vpn_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_termination_id")?;
        let item = state
            .repositories
            .l2vpn_terminations
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("L2VPN termination not found".into()))?;
        authorize_model(&state, &context, Action::Read, id, CONTENT_TYPE, &item).await?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path="/api/v1/inventory/l2vpn-terminations", tag="inventory", request_body=L2VpnTerminationCreatePayload, responses((status=201, description="Created", body=L2VpnTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_l2vpn_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<L2VpnTerminationCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Create).await?;
        validate_endpoint(&payload.assigned_object)?;
        let id = Uuid::now_v7();
        let outcome = validate_and_split_custom_fields(
            &state,
            CONTENT_TYPE,
            payload.custom_fields.map(serde_json::Value::Object),
            true,
        )
        .await?;
        let (fields, intents) = if let Some(outcome) = outcome {
            (outcome.custom_field_data, outcome.secret_intents)
        } else {
            (serde_json::json!({}), Vec::new())
        };
        let inputs = state
            .services
            .custom_field_values
            .prepare_secret_intents(id, intents)?;
        let item = NewL2VpnTermination {
            id,
            l2vpn_id: payload.l2vpn_id,
            assigned_object: payload.assigned_object,
            tags: payload.tags.unwrap_or_default(),
            custom_fields: fields,
        };
        let item = state
            .repositories
            .l2vpn_terminations
            .create_with_secret_intents(&item, inputs, |action, id, attrs| {
                authorize_attributes(&state, &context, action, id, attrs)
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

#[utoipa::path(patch, path="/api/v1/inventory/l2vpn-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=L2VpnTerminationPatchPayload, responses((status=200, description="Updated", body=L2VpnTermination), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_l2vpn_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<L2VpnTerminationPatchPayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_termination_id")?;
        if let Some(endpoint) = &payload.assigned_object {
            validate_endpoint(endpoint)?;
        }
        let mut changes = L2VpnTerminationChanges {
            l2vpn_id: payload.l2vpn_id,
            assigned_object: payload.assigned_object,
            tags: payload.tags,
            custom_fields: None,
        };
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
            .l2vpn_terminations
            .update_with_secret_intents(id, &changes, inputs, |action, id, attrs| {
                authorize_attributes(&state, &context, action, id, attrs)
            })
            .await?
            .ok_or_else(|| AppError::NotFound("L2VPN termination not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path="/api/v1/inventory/l2vpn-terminations/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_l2vpn_termination(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_termination_id")?;
        state
            .repositories
            .l2vpn_terminations
            .delete(id, |action, id, attrs| {
                authorize_attributes(&state, &context, action, id, attrs)
            })
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
