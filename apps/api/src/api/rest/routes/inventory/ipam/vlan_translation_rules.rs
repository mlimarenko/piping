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
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ipam::vlan_translation_rule::{CONTENT_TYPE, VlanTranslationRule};
use crate::contexts::inventory::infrastructure::persistence::repositories::ipam::vlan_translation_repository::{VLAN_TRANSLATION_RULE_COLUMNS, VlanTranslationRuleChanges, VlanTranslationRuleFilter};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids, search_pattern};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use super::vlan_translation_policies::authorize_vlan_translation_policy;
use crate::errors::AppError;
use super::super::{InventoryListEnvelope, masked_custom_fields, masked_custom_fields_page, parse_uuid, validate_and_split_custom_fields};

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VlanTranslationRuleCreatePayload {
    pub policy_id: Uuid,
    #[schema(minimum = 1, maximum = 4094)]
    pub local_vid: i16,
    #[schema(minimum = 1, maximum = 4094)]
    pub remote_vid: i16,
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
pub struct VlanTranslationRulePatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub policy_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, minimum = 1, maximum = 4094)]
    pub local_vid: Option<i16>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false, minimum = 1, maximum = 4094)]
    pub remote_vid: Option<i16>,
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
pub struct VlanTranslationRuleListQuery {
    pub search: Option<String>,
    pub policy_id: Option<String>,
    pub local_vid: Option<i16>,
    pub remote_vid: Option<i16>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Supported keys: policy_id, local_vid, remote_vid, description, created, last_updated.
    pub sort: Option<String>,
    pub order: Option<String>,
}

async fn mask(
    state: &AppState,
    mut item: VlanTranslationRule,
) -> Result<VlanTranslationRule, AppError> {
    item.custom_fields =
        masked_custom_fields(state, CONTENT_TYPE, item.id, &item.custom_fields).await?;
    Ok(item)
}

#[utoipa::path(get, path="/api/v1/inventory/vlan-translation-rules", tag="inventory", params(VlanTranslationRuleListQuery), responses((status=200, description="Rules", body=InventoryListEnvelope<VlanTranslationRule>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_vlan_translation_rules(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<VlanTranslationRuleListQuery>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Read).await?;
        let filter = VlanTranslationRuleFilter {
            search: search_pattern(query.search.as_deref()),
            policy_id: parse_csv_uuids(query.policy_id.as_deref(), "policy_id")?,
            local_vid: query.local_vid,
            remote_vid: query.remote_vid,
            sort: SortSpec::parse_columns(
                query.sort.as_deref(),
                query.order.as_deref(),
                VLAN_TRANSLATION_RULE_COLUMNS,
            )?,
        };
        let (mut items, total) = state
            .repositories
            .vlan_translation_rules
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

#[utoipa::path(get, path="/api/v1/inventory/vlan-translation-rules/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="Rule", body=VlanTranslationRule), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_vlan_translation_rule(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, auth, &headers);
    let result = async {
        let id = parse_uuid(&id, "vlan_translation_rule_id")?;
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
            .vlan_translation_rules
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("VLAN translation rule not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(post, path="/api/v1/inventory/vlan-translation-rules", tag="inventory", request_body=VlanTranslationRuleCreatePayload, responses((status=201, description="Created", body=VlanTranslationRule), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_vlan_translation_rule(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<VlanTranslationRuleCreatePayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &context, Action::Create).await?;
        authorize_vlan_translation_policy(&state, &context, Some(payload.policy_id)).await?;
        let mut item = VlanTranslationRule {
            id: Uuid::now_v7(),
            policy_id: payload.policy_id,
            local_vid: payload.local_vid,
            remote_vid: payload.remote_vid,
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
            .vlan_translation_rules
            .create_with_secret_intents(&item, inputs)
            .await?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(patch, path="/api/v1/inventory/vlan-translation-rules/{id}", tag="inventory", params(("id"=Uuid, Path)), request_body=VlanTranslationRulePatchPayload, responses((status=200, description="Updated", body=VlanTranslationRule), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_vlan_translation_rule(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<VlanTranslationRulePatchPayload>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "vlan_translation_rule_id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        if let Some(policy_id) = payload.policy_id {
            authorize_vlan_translation_policy(&state, &context, Some(policy_id)).await?;
        }
        let mut changes = VlanTranslationRuleChanges {
            policy_id: payload.policy_id,
            local_vid: payload.local_vid,
            remote_vid: payload.remote_vid,
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
            .vlan_translation_rules
            .update_with_secret_intents(id, &changes, inputs)
            .await?
            .ok_or_else(|| AppError::NotFound("VLAN translation rule not found".into()))?;
        mask(&state, item).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

#[utoipa::path(delete, path="/api/v1/inventory/vlan-translation-rules/{id}", tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_vlan_translation_rule(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "vlan_translation_rule_id")?;
        ensure_access_with_resource(
            &state,
            &context,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state.repositories.vlan_translation_rules.delete(id).await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(
            crate::api::rest::request_context::project_inventory_delete_error(
                &state, &context, error,
            )
            .await,
        ),
    }
}
