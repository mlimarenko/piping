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
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::vpn::l2vpn::{CONTENT_TYPE, L2Vpn, STATUSES, VPN_TYPES, parse_identifier};
use crate::contexts::inventory::infrastructure::persistence::repositories::vpn::l2vpn_repository::{L2VPN_COLUMNS, L2VpnChanges, L2VpnFilter};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, masked_custom_fields, masked_custom_fields_page, parse_uuid,
    validate_and_split_custom_fields,
};

fn active_status() -> String {
    "active".into()
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct L2VpnCreatePayload {
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    #[schema(min_length = 1, max_length = 100, pattern = "^[A-Za-z0-9_-]+$")]
    pub slug: String,
    #[serde(rename = "type")]
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn::type_schema)]
    pub vpn_type: String,
    #[serde(default = "active_status")]
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn::status_schema)]
    pub status: String,
    #[schema(pattern = "^(0|-?[1-9][0-9]*)$")]
    pub identifier: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    #[serde(default)]
    #[schema(max_length = 200)]
    pub description: String,
    #[serde(default)]
    pub comments: String,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub import_targets: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub export_targets: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct L2VpnPatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(
        nullable = false,
        min_length = 1,
        max_length = 100,
        pattern = "^[A-Za-z0-9_-]+$"
    )]
    pub slug: Option<String>,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "super::super::native::owners::present"
    )]
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn::type_schema)]
    pub vpn_type: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::vpn::l2vpn::status_schema)]
    pub status: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(value_type=Option<String>, pattern="^(0|-?[1-9][0-9]*)$")]
    pub identifier: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub tenant_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    pub owner_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, max_length = 200)]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub comments: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub import_targets: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub export_targets: Option<Vec<Uuid>>,
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
pub struct L2VpnListQuery {
    pub search: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub vpn_type: Option<String>,
    pub status: Option<String>,
    pub identifier: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_id: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Policy evaluation remains at the API boundary; repositories supply attributes
/// read inside their graph transaction, rather than trusting submitted owners.
pub(super) async fn authorize_attributes(
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

pub(super) async fn authorize_model<T: serde::Serialize>(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Uuid,
    token: &str,
    item: &T,
) -> Result<(), AppError> {
    let mut attributes = serde_json::to_value(item)?;
    attributes["object_type"] = serde_json::json!(token);
    authorize_attributes(state, context, action, id, attributes).await
}

async fn mask(state: &AppState, mut item: L2Vpn) -> Result<L2Vpn, AppError> {
    item.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, item.id, &item.custom_fields).await?;
    Ok(item)
}

#[utoipa::path(get, path="/api/v1/inventory/l2vpns", tag="inventory", params(L2VpnListQuery), responses((status=200, description="L2VPNs", body=InventoryListEnvelope<L2Vpn>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_l2vpns(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<L2VpnListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Read).await?;
        let vpn_type = parse_csv_strings(query.vpn_type.as_deref());
        let status = parse_csv_strings(query.status.as_deref());
        if vpn_type.iter().any(|v| !VPN_TYPES.contains(&v.as_str()))
            || status.iter().any(|v| !STATUSES.contains(&v.as_str()))
        {
            return Err(AppError::Validation(
                "unsupported L2VPN type or status filter".into(),
            ));
        }
        let filter = L2VpnFilter {
            search: search_pattern(query.search.as_deref()),
            name: parse_csv_strings(query.name.as_deref()),
            vpn_type,
            status,
            identifier: query
                .identifier
                .as_deref()
                .map(parse_identifier)
                .transpose()?,
            tenant_id: parse_csv_uuids(query.tenant_id.as_deref(), "tenant_id")?,
            owner_id: parse_csv_uuids(query.owner_id.as_deref(), "owner_id")?,
            sort: SortSpec::parse_columns(
                query.sort.as_deref(),
                query.order.as_deref(),
                L2VPN_COLUMNS,
            )?,
        };
        let (mut items, total) = state
            .repositories
            .l2vpns
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

#[utoipa::path(get, path="/api/v1/inventory/l2vpns/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="L2VPN", body=L2Vpn), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_l2vpn(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_id")?;
        let item = state
            .repositories
            .l2vpns
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("L2VPN not found".into()))?;
        authorize_model(&state, &context, Action::Read, id, CONTENT_TYPE, &item).await?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path="/api/v1/inventory/l2vpns", tag="inventory", request_body=L2VpnCreatePayload, responses((status=201, description="Created", body=L2Vpn), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_l2vpn(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<L2VpnCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Create).await?;
        let mut item = L2Vpn {
            id: Uuid::now_v7(),
            name: payload.name,
            slug: payload.slug,
            vpn_type: payload.vpn_type,
            status: payload.status,
            identifier: payload
                .identifier
                .as_deref()
                .map(parse_identifier)
                .transpose()?,
            tenant_id: payload.tenant_id,
            owner_id: payload.owner_id,
            description: payload.description,
            comments: payload.comments,
            import_targets: payload.import_targets.unwrap_or_default(),
            export_targets: payload.export_targets.unwrap_or_default(),
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
            .l2vpns
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

#[utoipa::path(patch, path="/api/v1/inventory/l2vpns/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=L2VpnPatchPayload, responses((status=200, description="Updated", body=L2Vpn), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_l2vpn(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<L2VpnPatchPayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_id")?;
        let mut changes = L2VpnChanges {
            name: payload.name,
            slug: payload.slug,
            vpn_type: payload.vpn_type,
            status: payload.status,
            identifier: payload
                .identifier
                .map(|v| v.as_deref().map(parse_identifier).transpose())
                .transpose()?,
            tenant_id: payload.tenant_id,
            owner_id: payload.owner_id,
            description: payload.description,
            comments: payload.comments,
            import_targets: payload.import_targets,
            export_targets: payload.export_targets,
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
            .l2vpns
            .update_with_secret_intents(id, &changes, inputs, |action, id, attrs| {
                authorize_attributes(&state, &context, action, id, attrs)
            })
            .await?
            .ok_or_else(|| AppError::NotFound("L2VPN not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path="/api/v1/inventory/l2vpns/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_l2vpn(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "l2vpn_id")?;
        state
            .repositories
            .l2vpns
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
