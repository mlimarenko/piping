//! ContactAssignments REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `ContactAssignmentRepository` on `AppState`.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::ObjectRef;
use crate::contexts::inventory::infrastructure::persistence::{
    BulkContactAssignmentResult, CONTACT_ASSIGNMENT_COLUMNS, ContactAssignmentFilter,
    ContactAssignmentRecord, ContactAssignmentRepository, NewContactAssignment,
    SqlxObjectRefResolver, UpdateContactAssignment,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_strings, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_uuid};

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactAssignmentListQuery {
    /// Case-insensitive free-text search over the entity display fields.
    pub search: Option<String>,
    pub content_type: Option<String>,
    pub object_id: Option<String>,
    pub role_id: Option<String>,
    pub contact_id: Option<String>,
    pub priority: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactAssignmentCreatePayload {
    /// Target content type in NetBox `app_label.model` form (e.g. `"dcim.site"`,
    /// `"tenancy.tenant"`, `"dcim.device"`). Resolved to a seeded content-type
    /// UUID server-side, so the caller does not need to know the random UUID.
    pub object_type: String,
    pub object_id: String,
    pub contact_id: String,
    pub role_id: String,
    pub priority: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactAssignmentUpdatePayload {
    /// Target content type in NetBox `app_label.model` form. When present it is
    /// re-resolved to a seeded content-type UUID server-side.
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub contact_id: Option<String>,
    pub role_id: Option<String>,
    pub priority: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactAssignmentBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ContactAssignmentCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContactAssignmentResponse {
    pub id: Uuid,
    /// Materialized polymorphic target. NOT NULL on this entity.
    pub object: ObjectRef,
    pub contact_id: Uuid,
    pub role_id: Uuid,
    pub priority: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkContactAssignmentResponse {
    pub created: Vec<ContactAssignmentResponse>,
    pub errors: Vec<String>,
}

/// Canonical NetBox-style contactable content types — the single vocabulary for
/// `object_type` on contact assignments. This MUST mirror the frontend
/// `GROUP_OBJECT_TYPE` map plus the `host` targets in
/// `apps/web/src/features/connections/components/connections/TreeNodeInspector.tsx`
/// (`attachTarget`); the mirror is locked by
/// `tests/compliance_contact_assignment_vocabulary.rs`.
///
/// Gating on this set prevents a typo'd or pluralised token (e.g. `"dcim.sites"`)
/// from reaching the write path, where it would fail the SELECT-only
/// `resolve_content_type` lookup against the seeded vocabulary.
///
/// This literal is the byte-stable source the text-parsing compliance test
/// reads; the unit test `contactable_literal_equals_registry_subset` asserts it
/// equals the registry-derived
/// [`crate::contexts::inventory::domain::content_type::CONTACT_ASSIGNMENT_OBJECT_TYPES`]
/// so the registry stays single-source-of-truth.
pub(crate) const CONTACTABLE_OBJECT_TYPES: &[&str] = &[
    "dcim.site",
    "dcim.region",
    "dcim.device",
    "tenancy.tenant",
    "virtualization.cluster",
    "virtualization.virtualmachine",
];

/// Split a NetBox-style `app_label.model` string (e.g. `"dcim.site"`) into its
/// two segments, rejecting anything that is not exactly two non-empty parts or
/// is not a canonical contactable content type (see [`CONTACTABLE_OBJECT_TYPES`]).
fn split_object_type(object_type: &str) -> Result<(String, String), AppError> {
    let trimmed = object_type.trim();
    let mut parts = trimmed.splitn(2, '.');
    match (parts.next(), parts.next()) {
        (Some(app_label), Some(model))
            if !app_label.trim().is_empty() && !model.trim().is_empty() && !model.contains('.') =>
        {
            let app_label = app_label.trim().to_string();
            let model = model.trim().to_string();
            let canonical = format!("{app_label}.{model}");
            if !CONTACTABLE_OBJECT_TYPES.contains(&canonical.as_str()) {
                return Err(AppError::Validation(format!(
                    "Unsupported object_type {object_type:?}: not a contactable content type. \
                     Expected one of: {}",
                    CONTACTABLE_OBJECT_TYPES.join(", ")
                )));
            }
            Ok((app_label, model))
        }
        _ => Err(AppError::Validation(format!(
            "Invalid object_type: expected \"app_label.model\" (e.g. \"dcim.site\"), got {object_type:?}"
        ))),
    }
}

fn map_contact_assignment(
    record: ContactAssignmentRecord,
    object: ObjectRef,
) -> ContactAssignmentResponse {
    ContactAssignmentResponse {
        id: record.id,
        object,
        contact_id: record.contact_id,
        role_id: record.role_id,
        priority: record.priority,
    }
}

/// Batch-resolve a page of records into responses with one resolver round-trip
/// (no N+1).
async fn map_contact_assignments(
    resolver: &SqlxObjectRefResolver,
    records: Vec<ContactAssignmentRecord>,
) -> Result<Vec<ContactAssignmentResponse>, AppError> {
    let pairs: Vec<(Uuid, Uuid)> = records
        .iter()
        .map(|record| (record.content_type, record.object_id))
        .collect();
    let refs = resolver.resolve(&pairs).await?;
    Ok(records
        .into_iter()
        .map(|record| {
            let object = refs
                .get(&(record.content_type, record.object_id))
                .cloned()
                .unwrap_or(ObjectRef {
                    object_type: String::new(),
                    object_id: record.object_id,
                    label: None,
                });
            map_contact_assignment(record, object)
        })
        .collect())
}

/// Resolve a single record into a response (get/create/update path).
async fn map_one_contact_assignment(
    resolver: &SqlxObjectRefResolver,
    record: ContactAssignmentRecord,
) -> Result<ContactAssignmentResponse, AppError> {
    let object = resolver
        .resolve_one(record.content_type, record.object_id)
        .await?;
    Ok(map_contact_assignment(record, object))
}

async fn map_bulk_contact_assignment_result(
    resolver: &SqlxObjectRefResolver,
    result: BulkContactAssignmentResult,
) -> Result<BulkContactAssignmentResponse, AppError> {
    Ok(BulkContactAssignmentResponse {
        created: map_contact_assignments(resolver, result.created).await?,
        errors: result.errors,
    })
}

/// List contact assignments.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-assignments",
    tag = "inventory",
    params(
        ("content_type" = Option<String>, Query, description = "Filter assignments by content-type UUID"),
        ("object_id" = Option<String>, Query, description = "Filter assignments by target entity UUID"),
        ("role_id" = Option<String>, Query, description = "Filter assignments by comma-separated contact role UUIDs"),
        ("contact_id" = Option<String>, Query, description = "Filter assignments by comma-separated contact UUIDs"),
        ("priority" = Option<String>, Query, description = "Filter assignments by comma-separated priority values"),
        ("limit" = Option<u32>, Query, description = "Maximum number of contact assignments to return"),
        ("offset" = Option<u32>, Query, description = "Number of contact assignments to skip"),
    ),
    responses(
        (status = 200, description = "Contact assignments", body = InventoryListEnvelope<ContactAssignmentResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_contact_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ContactAssignmentListQuery>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let role_ids = match parse_csv_uuids(query.role_id.as_deref(), "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let contact_ids = match parse_csv_uuids(query.contact_id.as_deref(), "contact_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let content_types = match parse_csv_uuids(query.content_type.as_deref(), "content_type") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let object_ids = match parse_csv_uuids(query.object_id.as_deref(), "object_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let priorities = parse_csv_strings(query.priority.as_deref());
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONTACT_ASSIGNMENT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .contact_assignments
        .list(
            ContactAssignmentFilter {
                content_types: &content_types,
                object_ids: &object_ids,
                role_ids: &role_ids,
                contact_ids: &contact_ids,
                priorities: &priorities,
                search: crate::contexts::shared::list_query::search_pattern(
                    query.search.as_deref(),
                ),
                sort,
            },
            query.limit,
            query.offset,
        )
        .await
    {
        Ok((records, total)) => {
            match map_contact_assignments(&state.repositories.object_ref_resolver, records).await {
                Ok(items) => (
                    StatusCode::OK,
                    Json(InventoryListEnvelope {
                        items,
                        total,
                        limit: resolved_limit,
                        offset: resolved_offset,
                    }),
                )
                    .into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a contact assignment.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contact-assignments/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact assignment ID"),
    ),
    responses(
        (status = 200, description = "Contact assignment", body = ContactAssignmentResponse),
        (status = 404, description = "Contact assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_contact_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Read,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.contact_assignments.get_by_id(id).await {
        Ok(Some(record)) => {
            match map_one_contact_assignment(&state.repositories.object_ref_resolver, record).await
            {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "ContactAssignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Create a contact assignment.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-assignments",
    tag = "inventory",
    request_body = ContactAssignmentCreatePayload,
    responses(
        (status = 201, description = "Contact assignment created", body = ContactAssignmentResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_contact_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactAssignmentCreatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let (app_label, model) = match split_object_type(&payload.object_type) {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let object_id = match parse_uuid(&payload.object_id, "object_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let contact_id = match parse_uuid(&payload.contact_id, "contact_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_uuid(&payload.role_id, "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let content_type = match state
        .repositories
        .contact_assignments
        .resolve_content_type(&app_label, &model)
        .await
    {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let new_assignment = NewContactAssignment {
        id: Uuid::now_v7(),
        content_type,
        object_id,
        contact_id,
        role_id,
        priority: payload.priority,
    };

    match state
        .repositories
        .contact_assignments
        .create(&new_assignment)
        .await
    {
        Ok(record) => {
            match map_one_contact_assignment(&state.repositories.object_ref_resolver, record).await
            {
                Ok(body) => (StatusCode::CREATED, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Update a contact assignment.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/contact-assignments/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact assignment ID"),
    ),
    request_body = ContactAssignmentUpdatePayload,
    responses(
        (status = 200, description = "Contact assignment updated", body = ContactAssignmentResponse),
        (status = 404, description = "Contact assignment not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_contact_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ContactAssignmentUpdatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let contact_id = match parse_optional_uuid(payload.contact_id, "contact_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let role_id = match parse_optional_uuid(payload.role_id, "role_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let content_type = match payload.object_type {
        Some(object_type) => {
            let (app_label, model) = match split_object_type(&object_type) {
                Ok(v) => v,
                Err(error) => return handle_app_error(error),
            };
            match state
                .repositories
                .contact_assignments
                .resolve_content_type(&app_label, &model)
                .await
            {
                Ok(v) => Some(v),
                Err(error) => return handle_app_error(error),
            }
        }
        None => None,
    };
    let object_id = match parse_optional_uuid(payload.object_id, "object_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateContactAssignment {
        content_type,
        object_id,
        contact_id,
        role_id,
        priority: payload.priority,
    };

    match state
        .repositories
        .contact_assignments
        .update(id, &changes)
        .await
    {
        Ok(Some(record)) => {
            match map_one_contact_assignment(&state.repositories.object_ref_resolver, record).await
            {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Ok(None) => handle_app_error(AppError::NotFound(
            "ContactAssignment not found".to_string(),
        )),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a contact assignment.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/contact-assignments/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact assignment ID"),
    ),
    responses(
        (status = 204, description = "Contact assignment deleted"),
    ),
)]
pub async fn delete_contact_assignment(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Delete,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }
    let id = match parse_uuid(&id, "id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    match state.repositories.contact_assignments.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create contact assignments.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contact-assignments/bulk",
    tag = "inventory",
    request_body = ContactAssignmentBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk contact assignment create result", body = BulkContactAssignmentResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_contact_assignments(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactAssignmentBulkCreatePayload>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Create,
        ResourceType::Inventory,
    )
    .await
    {
        return handle_app_error(error);
    }

    let mut items = Vec::new();
    for (index, p) in payload.items.into_iter().enumerate() {
        let (app_label, model) = match split_object_type(&p.object_type) {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let object_id = match parse_uuid(&p.object_id, "object_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let contact_id = match parse_uuid(&p.contact_id, "contact_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let role_id = match parse_uuid(&p.role_id, "role_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let content_type = match state
            .repositories
            .contact_assignments
            .resolve_content_type(&app_label, &model)
            .await
        {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewContactAssignment {
            id: Uuid::now_v7(),
            content_type,
            object_id,
            contact_id,
            role_id,
            priority: p.priority,
        });
    }

    match state
        .repositories
        .contact_assignments
        .bulk_create(&items)
        .await
    {
        Ok(result) => {
            match map_bulk_contact_assignment_result(
                &state.repositories.object_ref_resolver,
                result,
            )
            .await
            {
                Ok(body) => (StatusCode::OK, Json(body)).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::CONTACTABLE_OBJECT_TYPES;
    use crate::contexts::inventory::domain::content_type::{self, CONTACT_ASSIGNMENT_OBJECT_TYPES};

    /// The byte-stable literal here (which the text-parsing compliance test
    /// `compliance_contact_assignment_vocabulary.rs` mirror-locks to the FE)
    /// MUST equal the registry-derived contact subset. The literal cannot be a
    /// `const` re-export because that compliance test parses the source text of
    /// `CONTACTABLE_OBJECT_TYPES: &[&str] = &[...]`, so this runtime equality
    /// keeps the single-source-of-truth invariant.
    #[test]
    fn contactable_literal_equals_registry_subset() {
        assert_eq!(
            CONTACTABLE_OBJECT_TYPES, CONTACT_ASSIGNMENT_OBJECT_TYPES,
            "the literal CONTACTABLE_OBJECT_TYPES drifted from the registry \
             CONTACT_ASSIGNMENT_OBJECT_TYPES subset"
        );
    }

    /// Every contactable token must be a registered content type.
    #[test]
    fn contactable_tokens_are_registered() {
        for token in CONTACTABLE_OBJECT_TYPES {
            assert!(
                content_type::by_token(token).is_some(),
                "contactable token {token:?} is not in the content-type registry"
            );
        }
    }
}
