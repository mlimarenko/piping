use super::super::{
    InventoryListEnvelope, masked_custom_fields, masked_custom_fields_page, parse_uuid,
    validate_and_split_custom_fields,
};
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::{
    envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated},
    request_context::build_request_context,
};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::{
    circuits::circuit_group_assignment::{
        CONTENT_TYPE, CircuitGroupAssignment, normalize_priority, validate_member_type,
    },
    object_ref::ObjectRefInput,
};
use crate::contexts::inventory::infrastructure::persistence::{
    CircuitGroupRepository,
    repositories::circuits::circuit_group_assignment_repository::{
        CIRCUIT_GROUP_ASSIGNMENT_COLUMNS, CircuitGroupAssignmentChanges,
        CircuitGroupAssignmentFilter,
    },
};
use crate::contexts::shared::{
    list_query::{SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern},
    pagination::{clamp_limit, clamp_offset},
    request_context::RequestContext,
};
use crate::errors::AppError;
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CircuitGroupAssignmentCreatePayload {
    pub group_id: Uuid,
    #[schema(schema_with=crate::contexts::inventory::domain::circuits::circuit_group_assignment::member_input_schema)]
    pub member: ObjectRefInput,
    #[schema(schema_with=crate::contexts::inventory::domain::circuits::circuit_group_assignment::priority_input_schema)]
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CircuitGroupAssignmentPatchPayload {
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub group_id: Option<Uuid>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::circuits::circuit_group_assignment::member_input_schema)]
    pub member: Option<ObjectRefInput>,
    #[serde(
        default,
        deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch"
    )]
    #[schema(schema_with=crate::contexts::inventory::domain::circuits::circuit_group_assignment::priority_input_schema)]
    pub priority: Option<Option<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(schema_with=crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "super::super::native::owners::present")]
    #[schema(nullable = false)]
    pub custom_fields: Option<serde_json::Map<String, serde_json::Value>>,
}
impl From<CircuitGroupAssignmentCreatePayload> for CircuitGroupAssignmentChanges {
    fn from(p: CircuitGroupAssignmentCreatePayload) -> Self {
        Self {
            group_id: Some(p.group_id),
            member: Some(p.member),
            priority: Some(p.priority),
            tags: p.tags,
            custom_fields: p.custom_fields.map(serde_json::Value::Object),
        }
    }
}
impl From<CircuitGroupAssignmentPatchPayload> for CircuitGroupAssignmentChanges {
    fn from(p: CircuitGroupAssignmentPatchPayload) -> Self {
        Self {
            group_id: p.group_id,
            member: p.member,
            priority: p.priority,
            tags: p.tags,
            custom_fields: p.custom_fields.map(serde_json::Value::Object),
        }
    }
}
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct CircuitGroupAssignmentListQuery {
    pub group_id: Option<String>,
    pub member_id: Option<String>,
    pub member_type: Option<String>,
    pub priority: Option<String>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

async fn authorize_references(
    state: &AppState,
    ctx: &RequestContext,
    changes: &CircuitGroupAssignmentChanges,
) -> Result<(), AppError> {
    if let Some(group) = changes.group_id {
        ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{group}")),
        )
        .await?;
        if state
            .repositories
            .circuit_groups
            .get_by_id(group)
            .await?
            .is_none()
        {
            return Err(AppError::Validation("group_id does not exist".into()));
        }
    }
    if let Some(member) = &changes.member {
        validate_member_type(&member.object_type)?;
        ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{}", member.object_id)),
        )
        .await?;
    }
    if let Some(priority) = &changes.priority {
        normalize_priority(priority.clone())?;
    }
    Ok(())
}
async fn load_masked(state: &AppState, id: Uuid) -> Result<CircuitGroupAssignment, AppError> {
    let mut item = state
        .repositories
        .circuit_group_assignments
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Circuit group assignment not found".into()))?;
    item.custom_fields = masked_custom_fields(state, CONTENT_TYPE, id, &item.custom_fields).await?;
    Ok(item)
}
async fn write(
    state: &AppState,
    ctx: &RequestContext,
    id: Uuid,
    mut changes: CircuitGroupAssignmentChanges,
    create: bool,
) -> Result<CircuitGroupAssignment, AppError> {
    authorize_references(state, ctx, &changes).await?;
    let outcome =
        validate_and_split_custom_fields(state, CONTENT_TYPE, changes.custom_fields.take(), create)
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
    if create {
        state
            .repositories
            .circuit_group_assignments
            .create(id, &changes, inputs)
            .await?;
    } else if !state
        .repositories
        .circuit_group_assignments
        .update(id, &changes, inputs)
        .await?
    {
        return Err(AppError::NotFound(
            "Circuit group assignment not found".into(),
        ));
    }
    load_masked(state, id).await
}

/// List circuit group memberships with typed member and group filters.
#[utoipa::path(get,path="/api/v1/inventory/circuit-group-assignments",tag="inventory",params(CircuitGroupAssignmentListQuery),responses((status=200,description="Circuit group assignments",body=InventoryListEnvelope<CircuitGroupAssignment>),(status="4XX",description="Rejected",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",description="Failed",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn list_circuit_group_assignments(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(q): Query<CircuitGroupAssignmentListQuery>,
) -> Response {
    let ctx = build_request_context(&rid, auth, &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &ctx, Action::Read).await?;
        let member_type = parse_csv_strings(q.member_type.as_deref());
        for token in &member_type {
            validate_member_type(token)?;
        }
        let priority = parse_csv_strings(q.priority.as_deref());
        for value in &priority {
            normalize_priority(Some(value.clone()))?;
        }
        let filter = CircuitGroupAssignmentFilter {
            group_id: parse_csv_uuids(q.group_id.as_deref(), "group_id")?,
            member_id: parse_csv_uuids(q.member_id.as_deref(), "member_id")?,
            member_type,
            priority,
            search: search_pattern(q.search.as_deref()),
            sort: SortSpec::parse_columns(
                q.sort.as_deref(),
                q.order.as_deref(),
                CIRCUIT_GROUP_ASSIGNMENT_COLUMNS,
            )?,
        };
        let (mut items, total) = state
            .repositories
            .circuit_group_assignments
            .list(filter, q.limit, q.offset)
            .await?;
        let inputs = items
            .iter()
            .map(|item| (item.id, item.custom_fields.clone()))
            .collect();
        let masked = masked_custom_fields_page(&state, CONTENT_TYPE, inputs).await?;
        for (item, fields) in items.iter_mut().zip(masked) {
            item.custom_fields = fields;
        }
        Ok::<_, AppError>(InventoryListEnvelope {
            items,
            total,
            limit: clamp_limit(q.limit),
            offset: clamp_offset(q.offset),
        })
    }
    .await;
    match result {
        Ok(page) => Json(page).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Get one circuit group membership.
#[utoipa::path(get,path="/api/v1/inventory/circuit-group-assignments/{id}",tag="inventory",params(("id"=Uuid,Path)),responses((status=200,description="Circuit group assignment",body=CircuitGroupAssignment),(status="4XX",description="Rejected",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",description="Failed",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn get_circuit_group_assignment(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = build_request_context(&rid, auth, &headers);
    let result = async {
        let id = parse_uuid(&id, "circuit_group_assignment_id")?;
        ensure_access_with_resource(
            &state,
            &ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        load_masked(&state, id).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Create a circuit group membership.
#[utoipa::path(post,path="/api/v1/inventory/circuit-group-assignments",tag="inventory",request_body=CircuitGroupAssignmentCreatePayload,responses((status=201,description="Created",body=CircuitGroupAssignment),(status="4XX",description="Rejected",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",description="Failed",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn create_circuit_group_assignment(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CircuitGroupAssignmentCreatePayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        ensure_inventory_collection_access(&state, &ctx, Action::Create).await?;
        write(&state, &ctx, Uuid::now_v7(), payload.into(), true).await
    }
    .await;
    match result {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Partially update membership references or metadata; null clears priority only.
#[utoipa::path(patch,path="/api/v1/inventory/circuit-group-assignments/{id}",tag="inventory",params(("id"=Uuid,Path)),request_body=CircuitGroupAssignmentPatchPayload,responses((status=200,description="Updated",body=CircuitGroupAssignment),(status="4XX",description="Rejected",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",description="Failed",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn patch_circuit_group_assignment(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<CircuitGroupAssignmentPatchPayload>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "circuit_group_assignment_id")?;
        ensure_access_with_resource(
            &state,
            &ctx,
            Action::Update,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        write(&state, &ctx, id, payload.into(), false).await
    }
    .await;
    match result {
        Ok(item) => Json(item).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a circuit group membership through the shared secret/reference lifecycle.
#[utoipa::path(delete,path="/api/v1/inventory/circuit-group-assignments/{id}",tag="inventory",params(("id"=Uuid,Path)),responses((status=204,description="Deleted"),(status="4XX",description="Rejected",body=crate::api::rest::error_mapping::ApiErrorResponse),(status="5XX",description="Failed",body=crate::api::rest::error_mapping::ApiErrorResponse)))]
pub async fn delete_circuit_group_assignment(
    State(state): State<AppState>,
    Extension(rid): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = build_request_context(&rid, Some(auth), &headers);
    let result = async {
        let id = parse_uuid(&id, "circuit_group_assignment_id")?;
        ensure_access_with_resource(
            &state,
            &ctx,
            Action::Delete,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
        state
            .repositories
            .circuit_group_assignments
            .delete(id)
            .await
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}
