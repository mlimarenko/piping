//! Explicit native FHRP credential access; ordinary inventory responses never reveal it.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use uuid::Uuid;

use crate::api::middleware::{error::handle_app_error, request_id::RequestId};
use crate::api::rest::envelope_extractors::{Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::inventory::infrastructure::persistence::FhrpGroupRepository;
use crate::contexts::secrets::application::{
    SecretStore,
    consumer_type::{FHRP_AUTH_KEY_FIELD, INVENTORY_FHRP_GROUP_CONSUMER_TYPE},
};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct FhrpAuthKeyRevealResponse {
    pub value: String,
}

/// Authorize against the stored group and the token's concrete object scope.
pub(crate) async fn ensure_fhrp_access(
    state: &AppState,
    context: &RequestContext,
    id: Uuid,
    action: Action,
) -> Result<(), AppError> {
    let group = state
        .repositories
        .fhrp_groups
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("FHRP group not found".into()))?;
    let attributes = serde_json::to_value(&group)
        .map_err(|_| AppError::Internal("Cannot serialize FHRP attributes".into()))?;
    state
        .services
        .policy_evaluation
        .ensure_access_with_attributes(
            context,
            action,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
            attributes
                .as_object()
                .map(|map| map.clone().into_iter().collect()),
        )
        .await
}

#[utoipa::path(
    post, path = "/api/v1/inventory/fhrp-groups/{id}/auth-key/reveal",
    operation_id = "reveal_inventory_fhrp_auth_key", tag = "inventory",
    params(("id" = String, Path, description = "FHRP group ID")),
    responses(
        (status = 200, description = "FHRP auth key (empty when unset)", body = FhrpAuthKeyRevealResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Inventory read or secret access denied", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Group not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn reveal_fhrp_auth_key(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let context = build_request_context(&request_id, Some(auth), &headers);
    let result = async {
        let id = crate::api::rest::routes::validation::parse_uuid(&id, "fhrp_group_id")?;
        ensure_fhrp_access(&state, &context, id, Action::Read).await?;
        ensure_fhrp_access(&state, &context, id, Action::Secret).await?;
        let binding = state
            .repositories
            .secrets
            .get_usage_binding(INVENTORY_FHRP_GROUP_CONSUMER_TYPE, id, FHRP_AUTH_KEY_FIELD)
            .await?;
        let auth_key = match binding {
            Some(binding) => {
                if binding.owner_scope_type != INVENTORY_FHRP_GROUP_CONSUMER_TYPE
                    || binding.owner_scope_id != id
                {
                    return Err(AppError::Validation(
                        "FHRP secret binding ownership mismatch".into(),
                    ));
                }
                Some(
                    state
                        .services
                        .secret_store
                        .get_secret(binding.secret_id)
                        .await?,
                )
            }
            None => None,
        };
        state
            .services
            .audit
            .record_event(AuditEvent {
                id: Uuid::now_v7(),
                event_type: "inventory.fhrp_group.auth_key.reveal".into(),
                actor_id: Some(context.require_authenticated_user_id()?),
                actor_kind: context.audit_actor_kind().into(),
                target_type: Some(INVENTORY_FHRP_GROUP_CONSUMER_TYPE.into()),
                target_id: Some(id),
                metadata: serde_json::json!({"field_key": FHRP_AUTH_KEY_FIELD}),
                created_at: Utc::now(),
            })
            .await?;
        Ok::<_, AppError>(FhrpAuthKeyRevealResponse {
            value: auth_key.unwrap_or_default(),
        })
    }
    .await;
    match result {
        Ok(response) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(response),
        )
            .into_response(),
        Err(error) => handle_app_error(error),
    }
}
