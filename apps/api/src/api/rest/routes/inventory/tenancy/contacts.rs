//! Contacts REST routes — full CRUD.
//!
//! Handlers never run SQL directly; all persistence goes through `ContactRepository` on `AppState`.

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
use crate::contexts::inventory::infrastructure::persistence::{
    BulkCreateResult, CONTACT_COLUMNS, ContactRecord, ContactRepository, NewContact, UpdateContact,
};
use crate::contexts::shared::list_query::{SortSpec, parse_csv_uuids};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::AppError;

use super::super::{
    InventoryListEnvelope, ensure_access, parse_optional_uuid, parse_required_string, parse_uuid,
};

#[derive(Debug, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
#[into_params(parameter_in = Query)]
pub struct ContactListQuery {
    pub group_id: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactCreatePayload {
    pub name: String,
    pub group_id: Option<String>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactUpdatePayload {
    pub name: Option<String>,
    pub group_id: Option<String>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ContactBulkCreatePayload {
    #[serde(deserialize_with = "crate::api::rest::routes::validation::deserialize_bulk_items")]
    #[schema(min_items = 1)]
    pub items: Vec<ContactCreatePayload>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContactResponse {
    pub id: Uuid,
    pub name: String,
    pub group_id: Option<Uuid>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkContactResponse {
    pub created: Vec<ContactResponse>,
    pub errors: Vec<String>,
}

fn map_contact(record: ContactRecord) -> ContactResponse {
    ContactResponse {
        id: record.id,
        name: record.name,
        group_id: record.group_id,
        title: record.title,
        phone: record.phone,
        email: record.email,
        address: record.address,
        link: record.link,
        description: record.description,
    }
}

fn map_bulk_contact_result(result: BulkCreateResult<ContactRecord>) -> BulkContactResponse {
    BulkContactResponse {
        created: result.created.into_iter().map(map_contact).collect(),
        errors: result.errors,
    }
}

/// List contacts.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contacts",
    tag = "inventory",
    params(ContactListQuery),
    responses(
        (status = 200, description = "Contacts", body = InventoryListEnvelope<ContactResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_contacts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Query(query): Query<ContactListQuery>,
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

    let group_ids = match parse_csv_uuids(query.group_id.as_deref(), "group_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let resolved_limit = clamp_limit(query.limit);
    let resolved_offset = clamp_offset(query.offset);
    let sort = match SortSpec::parse_columns(
        query.sort.as_deref(),
        query.order.as_deref(),
        CONTACT_COLUMNS,
    ) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .contacts
        .list(&group_ids, query.search, query.limit, query.offset, sort)
        .await
    {
        Ok((records, total)) => {
            let items: Vec<ContactResponse> = records.into_iter().map(map_contact).collect();
            (
                StatusCode::OK,
                Json(InventoryListEnvelope {
                    items,
                    total,
                    limit: resolved_limit,
                    offset: resolved_offset,
                }),
            )
                .into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Get a contact.
#[utoipa::path(
    get,
    path = "/api/v1/inventory/contacts/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact ID"),
    ),
    responses(
        (status = 200, description = "Contact", body = ContactResponse),
        (status = 404, description = "Contact not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn get_contact(
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
    match state.repositories.contacts.get_by_id(id).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Contact not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Create a contact.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contacts",
    tag = "inventory",
    request_body = ContactCreatePayload,
    responses(
        (status = 201, description = "Contact created", body = ContactResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_contact(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactCreatePayload>,
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

    let name = match parse_required_string(&payload.name, "name") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };
    let group_id = match parse_optional_uuid(payload.group_id, "group_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let new_contact = NewContact {
        id: Uuid::now_v7(),
        name,
        group_id,
        title: payload.title,
        phone: payload.phone,
        email: payload.email,
        address: payload.address,
        link: payload.link,
        description: payload.description,
    };

    match state.repositories.contacts.create(&new_contact).await {
        Ok(record) => (StatusCode::CREATED, Json(map_contact(record))).into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Update a contact.
#[utoipa::path(
    put,
    path = "/api/v1/inventory/contacts/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact ID"),
    ),
    request_body = ContactUpdatePayload,
    responses(
        (status = 200, description = "Contact updated", body = ContactResponse),
        (status = 404, description = "Contact not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_contact(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
    EnvelopeJson(payload): EnvelopeJson<ContactUpdatePayload>,
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
    let group_id = match parse_optional_uuid(payload.group_id, "group_id") {
        Ok(v) => v,
        Err(error) => return handle_app_error(error),
    };

    let changes = UpdateContact {
        name: payload.name,
        group_id,
        title: payload.title,
        phone: payload.phone,
        email: payload.email,
        address: payload.address,
        link: payload.link,
        description: payload.description,
    };

    match state.repositories.contacts.update(id, &changes).await {
        Ok(Some(record)) => (StatusCode::OK, Json(map_contact(record))).into_response(),
        Ok(None) => handle_app_error(AppError::NotFound("Contact not found".to_string())),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a contact.
#[utoipa::path(
    delete,
    path = "/api/v1/inventory/contacts/{id}",
    tag = "inventory",
    params(
        ("id" = String, Path, description = "Contact ID"),
    ),
    responses(
        (status = 204, description = "Contact deleted"),
    ),
)]
pub async fn delete_contact(
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
    match state.repositories.contacts.delete(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => handle_app_error(error),
    }
}

/// Bulk-create contacts.
#[utoipa::path(
    post,
    path = "/api/v1/inventory/contacts/bulk",
    tag = "inventory",
    request_body = ContactBulkCreatePayload,
    responses(
        (status = 200, description = "Bulk contact create result", body = BulkContactResponse),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_create_contacts(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<ContactBulkCreatePayload>,
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
        let name = match parse_required_string(&p.name, "name") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        let group_id = match parse_optional_uuid(p.group_id, "group_id") {
            Ok(v) => v,
            Err(error) => {
                return handle_app_error(AppError::Validation(format!("item {index}: {error:?}")));
            }
        };
        items.push(NewContact {
            id: Uuid::now_v7(),
            name,
            group_id,
            title: p.title,
            phone: p.phone,
            email: p.email,
            address: p.address,
            link: p.link,
            description: p.description,
        });
    }

    match state.repositories.contacts.bulk_create(&items).await {
        Ok(result) => (StatusCode::OK, Json(map_bulk_contact_result(result))).into_response(),
        Err(error) => handle_app_error(error),
    }
}
