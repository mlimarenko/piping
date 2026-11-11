use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::inventory::application::{
    FullBackupArchive, FullBackupExportRequest, FullBackupImportRequest, FullBackupImportSummary,
    full_backup_import_requires_secret_permission,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;
use crate::observability::{EventFields, log_error_event};

use super::super::{bump_host_config_cache_generation, ensure_access};

/// Export a full instance backup.
#[utoipa::path(
    post,
    path = "/api/v1/admin/backup/export",
    tag = "administration",
    request_body = FullBackupExportRequest,
    responses(
        (status = 200, description = "Full instance backup payload", body = FullBackupArchive),
        (status = "4XX", description = "Backup export request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Backup export failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn export_full_backup(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<FullBackupExportRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) =
        ensure_access(&state, &request_context, Action::Read, ResourceType::Backup).await
    {
        return handle_app_error(error);
    }
    if request.include_secrets {
        if !request.confirm_secret_export {
            return handle_app_error(AppError::Validation(
                "Secret backup export must be explicitly confirmed".to_string(),
            ));
        }
        if let Err(error) = ensure_access(
            &state,
            &request_context,
            Action::Secret,
            ResourceType::SecretVariable,
        )
        .await
        {
            return handle_app_error(error);
        }
    }
    let service = state.services.backup.clone();
    match service
        .export_full_backup(request.include_secrets, request.selection)
        .await
    {
        Ok(payload) => {
            if let Err(error) =
                record_backup_audit(&state, &request_context, "admin.backup.exported", &payload)
                    .await
            {
                return handle_app_error(error);
            }
            (StatusCode::OK, Json(payload)).into_response()
        }
        Err(error) => handle_app_error(error),
    }
}

/// Restore a full instance backup.
#[utoipa::path(
    post,
    path = "/api/v1/admin/backup/import",
    tag = "administration",
    request_body = FullBackupImportRequest,
    responses(
        (status = 200, description = "Full instance restore summary", body = FullBackupImportSummary),
        (status = "4XX", description = "Backup import request was rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Backup import failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn import_full_backup(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(request): EnvelopeJson<FullBackupImportRequest>,
) -> impl IntoResponse {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    if let Err(error) = ensure_access(
        &state,
        &request_context,
        Action::Update,
        ResourceType::Backup,
    )
    .await
    {
        return handle_app_error(error);
    }
    let requires_secret_permission = match full_backup_import_requires_secret_permission(&request) {
        Ok(value) => value,
        Err(error) => return handle_app_error(error),
    };
    if requires_secret_permission
        && let Err(error) = ensure_access(
            &state,
            &request_context,
            Action::Update,
            ResourceType::SecretVariable,
        )
        .await
    {
        return handle_app_error(error);
    }
    let service = state.services.backup.clone();
    match service.import_full_backup(request).await {
        Ok(summary) => {
            if summary.applied {
                bump_host_config_cache_generation(&state).await;
            }
            if let Err(error) = record_backup_import_audit(&state, &request_context, &summary).await
            {
                log_error_event(
                    "rest.admin.backup.import.audit_failed",
                    "admin_backup_import_audit_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("rest.admin.backup.import.audit")
                        .with_resource("admin_backup", "payload")
                        .with_message("Backup import completed but audit recording failed"),
                );
            }
            (StatusCode::OK, Json(summary)).into_response()
        }
        Err(error) => {
            log_error_event(
                "rest.admin.backup.import.failed",
                "admin_backup_import_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("rest.admin.backup.import")
                    .with_resource("admin_backup", "payload")
                    .with_message("Full backup import failed"),
            );
            handle_app_error(error)
        }
    }
}

fn backup_actor_id(request_context: &RequestContext) -> Option<Uuid> {
    request_context.user_id
}

fn backup_actor_kind(request_context: &RequestContext) -> String {
    if request_context.api_token_id.is_some() {
        "api_token".to_string()
    } else {
        "user".to_string()
    }
}

async fn record_backup_audit(
    state: &AppState,
    request_context: &RequestContext,
    event_type: &str,
    payload: &FullBackupArchive,
) -> Result<(), AppError> {
    state
        .services
        .audit
        .record_event(AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            actor_id: backup_actor_id(request_context),
            actor_kind: backup_actor_kind(request_context),
            target_type: Some("admin_backup".to_string()),
            target_id: None,
            metadata: json!({
                "auth_method": request_context.auth_method.clone(),
                "api_token_id": request_context.api_token_id.clone(),
                "include_secrets": payload.include_secrets,
                "schema_version": payload.schema_version,
                "table_count": payload.table_count,
                "row_count": payload.row_count,
                "skipped_count": payload.skipped.len(),
            }),
            created_at: Utc::now(),
        })
        .await
}

async fn record_backup_import_audit(
    state: &AppState,
    request_context: &RequestContext,
    summary: &FullBackupImportSummary,
) -> Result<(), AppError> {
    let event_type = if summary.applied {
        "admin.backup.restored"
    } else {
        "admin.backup.restore_previewed"
    };
    state
        .services
        .audit
        .record_event(AuditEvent {
            id: Uuid::now_v7(),
            event_type: event_type.to_string(),
            actor_id: backup_actor_id(request_context),
            actor_kind: backup_actor_kind(request_context),
            target_type: Some("admin_backup".to_string()),
            target_id: None,
            metadata: json!({
                "auth_method": request_context.auth_method.clone(),
                "api_token_id": request_context.api_token_id.clone(),
                "table_count": summary.table_count,
                "row_count": summary.row_count,
                "mode": summary.mode,
                "applied": summary.applied,
                "dry_run": summary.plan.as_ref().is_some_and(|plan| plan.dry_run),
                "has_destructive_changes": summary
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.has_destructive_changes),
            }),
            created_at: Utc::now(),
        })
        .await
}
