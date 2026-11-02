use std::future::Future;

use axum::{
    Extension, Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

use super::{
    BulkError, BulkRequest, BulkResponse, BulkUpdate, DomainGroup, DomainPolicy, DomainRole,
    DomainUser, GroupInput, GroupResponse, PolicyInput, PolicyResponse, RoleInput, RoleResponse,
    UserInput, UserResponse, build_policy_from_input, build_user, ensure_access, parse_uuid,
};

/// Rejection message shared by the create and update phases of the user route.
const CREDENTIAL_NOT_SUPPORTED_IN_BULK: &str =
    "Credential operations are not supported in bulk user requests";

async fn ensure_bulk_permissions<T>(
    state: &AppState,
    request_context: &RequestContext,
    payload: &BulkRequest<T>,
    resource_type: ResourceType,
) -> Result<(), AppError> {
    if payload
        .create
        .as_ref()
        .is_some_and(|items| !items.is_empty())
    {
        ensure_access(
            state,
            request_context,
            Action::Create,
            resource_type.clone(),
        )
        .await?;
    }
    if payload
        .update
        .as_ref()
        .is_some_and(|items| !items.is_empty())
    {
        ensure_access(
            state,
            request_context,
            Action::Update,
            resource_type.clone(),
        )
        .await?;
    }
    if payload
        .delete
        .as_ref()
        .is_some_and(|items| !items.is_empty())
    {
        ensure_access(state, request_context, Action::Delete, resource_type).await?;
    }
    Ok(())
}

/// Empty batch accumulator; the concrete response element type is fixed by the
/// caller's annotation.
fn empty_bulk_response<R>() -> BulkResponse<R> {
    BulkResponse {
        created: Vec::new(),
        updated: Vec::new(),
        deleted: Vec::new(),
        errors: Vec::new(),
    }
}

/// Record one rejected item without aborting the rest of the batch.
fn push_bulk_error(errors: &mut Vec<BulkError>, id: Option<String>, error: impl ToString) {
    errors.push(BulkError {
        id,
        error: error.to_string(),
    });
}

/// Resolve one bulk item id, recording a parse error and returning `None` so the
/// caller can skip the item.
fn resolve_bulk_id(raw: &str, errors: &mut Vec<BulkError>) -> Option<Uuid> {
    match parse_uuid(raw, "id") {
        Ok(id) => Some(id),
        Err(error) => {
            push_bulk_error(errors, Some(raw.to_string()), error);
            None
        }
    }
}

/// Shared create phase of every bulk route: build the domain entity, persist it,
/// then record either the mapped response or the item error.
async fn apply_bulk_create<T, D, R, Build, Create, Fut>(
    items: Vec<T>,
    response: &mut BulkResponse<R>,
    build: Build,
    create: Create,
) where
    T: Send,
    D: Send,
    R: Send + From<D>,
    Build: Fn(T) -> Result<D, AppError> + Send,
    Create: Fn(D) -> Fut + Send,
    Fut: Future<Output = Result<D, AppError>> + Send,
{
    for item in items {
        let domain = match build(item) {
            Ok(domain) => domain,
            Err(error) => {
                push_bulk_error(&mut response.errors, None, error);
                continue;
            }
        };
        match create(domain).await {
            Ok(domain) => response.created.push(R::from(domain)),
            Err(error) => push_bulk_error(&mut response.errors, None, error),
        }
    }
}

/// Shared update phase of every bulk route: resolve the target id, rebuild the
/// domain entity with that id, persist it and record the outcome per item.
async fn apply_bulk_update<T, D, R, Build, Update, Fut>(
    items: Vec<BulkUpdate<T>>,
    response: &mut BulkResponse<R>,
    build: Build,
    update: Update,
) where
    T: Send,
    D: Send,
    R: Send + From<D>,
    Build: Fn(T, Uuid) -> Result<D, AppError> + Send,
    Update: Fn(D) -> Fut + Send,
    Fut: Future<Output = Result<D, AppError>> + Send,
{
    for item in items {
        let Some(target_id) = resolve_bulk_id(&item.id, &mut response.errors) else {
            continue;
        };
        let domain = match build(item.data, target_id) {
            Ok(domain) => domain,
            Err(error) => {
                push_bulk_error(&mut response.errors, Some(target_id.to_string()), error);
                continue;
            }
        };
        match update(domain).await {
            Ok(domain) => response.updated.push(R::from(domain)),
            Err(error) => {
                push_bulk_error(&mut response.errors, Some(target_id.to_string()), error);
            }
        }
    }
}

/// Shared delete phase of every bulk route: resolve the target id, delete it and
/// record either the id or a `not_found` / service error per item.
async fn apply_bulk_delete<R, Delete, Fut>(
    items: Vec<String>,
    response: &mut BulkResponse<R>,
    not_found: &str,
    delete: Delete,
) where
    R: Send,
    Delete: Fn(Uuid) -> Fut + Send,
    Fut: Future<Output = Result<bool, AppError>> + Send,
{
    for item in items {
        let Some(target_id) = resolve_bulk_id(&item, &mut response.errors) else {
            continue;
        };
        match delete(target_id).await {
            Ok(true) => response.deleted.push(target_id.to_string()),
            Ok(false) => {
                push_bulk_error(&mut response.errors, Some(target_id.to_string()), not_found);
            }
            Err(error) => {
                push_bulk_error(&mut response.errors, Some(target_id.to_string()), error);
            }
        }
    }
}

/// Apply a bulk create/update/delete operation to users.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/users/bulk",
    tag = "access-control",
    request_body = BulkRequest<UserInput>,
    responses(
        (status = 200, description = "Bulk user result", body = BulkResponse<UserResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_users(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkRequest<UserInput>>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_bulk_permissions(&state, &request_context, &payload, ResourceType::User).await
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    let mut response: BulkResponse<UserResponse> = empty_bulk_response();

    if let Some(create) = payload.create {
        apply_bulk_create(
            create,
            &mut response,
            |item: UserInput| -> Result<DomainUser, AppError> {
                if item.credential.is_some() {
                    return Err(AppError::Validation(
                        CREDENTIAL_NOT_SUPPORTED_IN_BULK.to_string(),
                    ));
                }
                build_user(&item, Uuid::now_v7())
            },
            |user: DomainUser| {
                let service = service.clone();
                async move { service.create_user(user).await }
            },
        )
        .await;
    }

    if let Some(update) = payload.update {
        apply_bulk_update(
            update,
            &mut response,
            |data: UserInput, id: Uuid| -> Result<DomainUser, AppError> {
                if data.credential.is_some() {
                    return Err(AppError::Validation(
                        CREDENTIAL_NOT_SUPPORTED_IN_BULK.to_string(),
                    ));
                }
                build_user(&data, id)
            },
            |user: DomainUser| {
                let service = service.clone();
                async move { service.update_user(user).await }
            },
        )
        .await;
    }

    if let Some(delete) = payload.delete {
        apply_bulk_delete(delete, &mut response, "User not found", |id: Uuid| {
            let service = service.clone();
            async move { service.delete_user(id).await }
        })
        .await;
    }

    Json(response).into_response()
}

/// Apply a bulk create/update/delete operation to groups.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/groups/bulk",
    tag = "access-control",
    request_body = BulkRequest<GroupInput>,
    responses(
        (status = 200, description = "Bulk group result", body = BulkResponse<GroupResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_groups(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkRequest<GroupInput>>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_bulk_permissions(&state, &request_context, &payload, ResourceType::Group).await
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    let mut response: BulkResponse<GroupResponse> = empty_bulk_response();

    if let Some(create) = payload.create {
        apply_bulk_create(
            create,
            &mut response,
            |item: GroupInput| -> Result<DomainGroup, AppError> {
                Ok(DomainGroup {
                    id: Uuid::now_v7(),
                    name: item.name,
                    external_id: item.external_id,
                })
            },
            |group: DomainGroup| {
                let service = service.clone();
                async move { service.create_group(group).await }
            },
        )
        .await;
    }

    if let Some(update) = payload.update {
        apply_bulk_update(
            update,
            &mut response,
            |data: GroupInput, id: Uuid| -> Result<DomainGroup, AppError> {
                Ok(DomainGroup {
                    id,
                    name: data.name,
                    external_id: data.external_id,
                })
            },
            |group: DomainGroup| {
                let service = service.clone();
                async move { service.update_group(group).await }
            },
        )
        .await;
    }

    if let Some(delete) = payload.delete {
        apply_bulk_delete(delete, &mut response, "Group not found", |id: Uuid| {
            let service = service.clone();
            async move { service.delete_group(id).await }
        })
        .await;
    }

    Json(response).into_response()
}

/// Apply a bulk create/update/delete operation to roles.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/roles/bulk",
    tag = "access-control",
    request_body = BulkRequest<RoleInput>,
    responses(
        (status = 200, description = "Bulk role result", body = BulkResponse<RoleResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_roles(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkRequest<RoleInput>>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_bulk_permissions(&state, &request_context, &payload, ResourceType::Role).await
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    let mut response: BulkResponse<RoleResponse> = empty_bulk_response();

    if let Some(create) = payload.create {
        apply_bulk_create(
            create,
            &mut response,
            |item: RoleInput| -> Result<DomainRole, AppError> {
                Ok(DomainRole {
                    id: Uuid::now_v7(),
                    name: item.name,
                    description: item.description,
                })
            },
            |role: DomainRole| {
                let service = service.clone();
                async move { service.create_role(role).await }
            },
        )
        .await;
    }

    if let Some(update) = payload.update {
        apply_bulk_update(
            update,
            &mut response,
            |data: RoleInput, id: Uuid| -> Result<DomainRole, AppError> {
                Ok(DomainRole {
                    id,
                    name: data.name,
                    description: data.description,
                })
            },
            |role: DomainRole| {
                let service = service.clone();
                async move { service.update_role(role).await }
            },
        )
        .await;
    }

    if let Some(delete) = payload.delete {
        apply_bulk_delete(delete, &mut response, "Role not found", |id: Uuid| {
            let service = service.clone();
            async move { service.delete_role(id).await }
        })
        .await;
    }

    Json(response).into_response()
}

/// Apply a bulk create/update/delete operation to policies.
#[utoipa::path(
    post,
    path = "/api/v1/admin/access-control/policies/bulk",
    tag = "access-control",
    request_body = BulkRequest<PolicyInput>,
    responses(
        (status = 200, description = "Bulk policy result", body = BulkResponse<PolicyResponse>),
        (status = 400, description = "Bad request", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn bulk_policies(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<BulkRequest<PolicyInput>>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = payload.validate() {
        return handle_app_error(error);
    }
    if let Err(error) =
        ensure_bulk_permissions(&state, &request_context, &payload, ResourceType::Policy).await
    {
        return handle_app_error(error);
    }

    let service = state.services.access_control_admin.clone();
    let mut response: BulkResponse<PolicyResponse> = empty_bulk_response();

    if let Some(create) = payload.create {
        apply_bulk_create(
            create,
            &mut response,
            |item: PolicyInput| build_policy_from_input(item, Uuid::now_v7()),
            |policy: DomainPolicy| {
                let service = service.clone();
                async move { service.create_policy(policy).await }
            },
        )
        .await;
    }

    if let Some(update) = payload.update {
        apply_bulk_update(
            update,
            &mut response,
            |data: PolicyInput, id: Uuid| build_policy_from_input(data, id),
            |policy: DomainPolicy| {
                let service = service.clone();
                async move { service.update_policy(policy).await }
            },
        )
        .await;
    }

    if let Some(delete) = payload.delete {
        apply_bulk_delete(delete, &mut response, "Policy not found", |id: Uuid| {
            let service = service.clone();
            async move { service.delete_policy(id).await.map(|()| true) }
        })
        .await;
    }

    Json(response).into_response()
}
