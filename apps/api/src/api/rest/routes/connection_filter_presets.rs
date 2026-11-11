//! REST handlers for public saved connection filter presets.

use std::collections::BTreeMap;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::auth::AuthContext;
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, RequireAuthenticated};
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::connections::infrastructure::persistence::ConnectionFilterPreset;
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

const FILTER_VALUE_MAX_BYTES: usize = 16 * 1024;
const FILTER_STRING_MAX_CHARS: usize = 256;
const PUBLIC_FILTER_PRESET_LIST_LIMIT: i64 = 200;
const FILTER_KEYS: &[&str] = &[
    "status",
    "region",
    "site",
    "tenant",
    "role",
    "platform",
    "deviceType",
    "cluster",
];

type FilterPresetFilters = BTreeMap<String, Vec<String>>;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateConnectionFilterPresetRequest {
    pub name: String,
    pub filters: FilterPresetFilters,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateConnectionFilterPresetRequest {
    pub name: String,
    pub filters: FilterPresetFilters,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ConnectionFilterPresetResponse {
    pub id: Uuid,
    pub name: String,
    pub filters: FilterPresetFilters,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListConnectionFilterPresetsResponse {
    pub items: Vec<ConnectionFilterPresetResponse>,
}

impl TryFrom<ConnectionFilterPreset> for ConnectionFilterPresetResponse {
    type Error = AppError;

    fn try_from(preset: ConnectionFilterPreset) -> Result<Self, Self::Error> {
        Ok(Self {
            id: preset.id,
            name: preset.name,
            filters: filters_from_value(preset.filters)?,
            created_by: preset.created_by,
            created_at: preset.created_at,
            updated_at: preset.updated_at,
        })
    }
}

/// List public connection filter presets.
#[utoipa::path(
    get,
    path = "/api/v1/connections/filter-presets",
    tag = "connections",
    responses(
        (status = 200, description = "Public connection filter presets", body = ListConnectionFilterPresetsResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn list_connection_filter_presets(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    if let Err(error) = request_context.require_authenticated_user_id() {
        return handle_app_error(error);
    }

    match state
        .repositories
        .connection_filter_presets
        .list(PUBLIC_FILTER_PRESET_LIST_LIMIT)
        .await
    {
        Ok(items) => {
            let response_items = items
                .into_iter()
                .map(ConnectionFilterPresetResponse::try_from)
                .collect::<Result<Vec<_>, AppError>>();
            match response_items {
                Ok(items) => Json(ListConnectionFilterPresetsResponse { items }).into_response(),
                Err(error) => handle_app_error(error),
            }
        }
        Err(error) => handle_app_error(error),
    }
}

/// Create a public connection filter preset.
#[utoipa::path(
    post,
    path = "/api/v1/connections/filter-presets",
    tag = "connections",
    request_body = CreateConnectionFilterPresetRequest,
    responses(
        (status = 201, description = "Connection filter preset created", body = ConnectionFilterPresetResponse),
        (status = 400, description = "Invalid preset", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Preset name already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn create_connection_filter_preset(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    EnvelopeJson(payload): EnvelopeJson<CreateConnectionFilterPresetRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };
    let name = match validate_name(&payload.name) {
        Ok(name) => name,
        Err(error) => return handle_app_error(error),
    };
    let filters = match validate_filters(&payload.filters) {
        Ok(filters) => filters,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .connection_filter_presets
        .create(Uuid::now_v7(), &name, &filters, user_id)
        .await
    {
        Ok(preset) => match ConnectionFilterPresetResponse::try_from(preset) {
            Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
            Err(error) => handle_app_error(error),
        },
        Err(error) => handle_app_error(error),
    }
}

/// Update a public connection filter preset.
#[utoipa::path(
    put,
    path = "/api/v1/connections/filter-presets/{id}",
    tag = "connections",
    params(("id" = Uuid, Path, description = "Preset id")),
    request_body = UpdateConnectionFilterPresetRequest,
    responses(
        (status = 200, description = "Connection filter preset updated", body = ConnectionFilterPresetResponse),
        (status = 400, description = "Invalid preset", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Owner or admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Preset not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 409, description = "Preset name already exists", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn update_connection_filter_preset(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    RequireAuthenticated(auth_context): RequireAuthenticated,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    EnvelopeJson(payload): EnvelopeJson<UpdateConnectionFilterPresetRequest>,
) -> Response {
    let request_context = build_request_context(&request_id, Some(auth_context), &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };

    if let Err(error) =
        ensure_owner_or_admin(&state, &request_context, id, user_id, Action::Update).await
    {
        return handle_app_error(error);
    }
    let name = match validate_name(&payload.name) {
        Ok(name) => name,
        Err(error) => return handle_app_error(error),
    };
    let filters = match validate_filters(&payload.filters) {
        Ok(filters) => filters,
        Err(error) => return handle_app_error(error),
    };

    match state
        .repositories
        .connection_filter_presets
        .update(id, &name, &filters)
        .await
    {
        Ok(Some(preset)) => match ConnectionFilterPresetResponse::try_from(preset) {
            Ok(response) => Json(response).into_response(),
            Err(error) => handle_app_error(error),
        },
        Ok(None) => handle_app_error(AppError::NotFound(format!(
            "Connection filter preset '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

/// Delete a public connection filter preset.
#[utoipa::path(
    delete,
    path = "/api/v1/connections/filter-presets/{id}",
    tag = "connections",
    params(("id" = Uuid, Path, description = "Preset id")),
    responses(
        (status = 204, description = "Connection filter preset deleted"),
        (status = 401, description = "Authentication required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 403, description = "Owner or admin access required", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = 404, description = "Preset not found", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn delete_connection_filter_preset(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    auth_context: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let request_context = build_request_context(&request_id, auth_context, &headers);
    let user_id = match request_context.require_authenticated_user_id() {
        Ok(user_id) => user_id,
        Err(error) => return handle_app_error(error),
    };

    if let Err(error) =
        ensure_owner_or_admin(&state, &request_context, id, user_id, Action::Delete).await
    {
        return handle_app_error(error);
    }

    match state
        .repositories
        .connection_filter_presets
        .delete(id)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => handle_app_error(AppError::NotFound(format!(
            "Connection filter preset '{id}' not found"
        ))),
        Err(error) => handle_app_error(error),
    }
}

async fn ensure_owner_or_admin(
    state: &AppState,
    request_context: &RequestContext,
    preset_id: Uuid,
    user_id: Uuid,
    admin_action: Action,
) -> Result<(), AppError> {
    let preset = state
        .repositories
        .connection_filter_presets
        .get(preset_id)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("Connection filter preset '{preset_id}' not found"))
        })?;

    if preset.created_by == user_id {
        return Ok(());
    }

    state
        .services
        .policy_evaluation
        .ensure_access(
            request_context,
            admin_action,
            ResourceType::ConnectionFilterPreset,
            Some("public"),
        )
        .await
}

fn validate_name(name: &str) -> Result<String, AppError> {
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    if !(1..=80).contains(&len) {
        return Err(AppError::Validation(
            "connection filter preset name must be between 1 and 80 characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn filters_from_value(value: Value) -> Result<FilterPresetFilters, AppError> {
    serde_json::from_value(value).map_err(|error| {
        AppError::Database(format!(
            "Failed to decode connection filter preset filters: {error}"
        ))
    })
}

fn validate_filters(filters: &FilterPresetFilters) -> Result<Value, AppError> {
    let value = serde_json::to_value(filters).map_err(AppError::ValidationJson)?;
    let bytes = serde_json::to_vec(&value).map_err(AppError::ValidationJson)?;
    if bytes.len() > FILTER_VALUE_MAX_BYTES {
        return Err(AppError::Validation(
            "connection filter preset filters exceed 16 KiB limit".to_string(),
        ));
    }

    if filters.is_empty() {
        return Err(AppError::Validation(
            "connection filter preset must contain at least one filter".to_string(),
        ));
    }

    for (key, values) in filters {
        if !FILTER_KEYS.contains(&key.as_str()) {
            return Err(AppError::Validation(format!(
                "unsupported connection filter key '{key}'"
            )));
        }
        if values.is_empty() {
            return Err(AppError::Validation(format!(
                "connection filter '{key}' must contain at least one value"
            )));
        }
        for filter_value in values {
            if filter_value.is_empty() || filter_value.chars().count() > FILTER_STRING_MAX_CHARS {
                return Err(AppError::Validation(format!(
                    "connection filter '{key}' values must be 1..{FILTER_STRING_MAX_CHARS} characters"
                )));
            }
        }
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_filters() -> FilterPresetFilters {
        BTreeMap::from([("status".to_string(), vec!["active".to_string()])])
    }

    #[test]
    fn validate_name_trims_and_bounds_length() {
        assert_eq!(validate_name("  Active sites  ").unwrap(), "Active sites");
        assert!(validate_name("").is_err());
        assert!(validate_name(" ".repeat(81).as_str()).is_err());
        assert!(validate_name("x".repeat(81).as_str()).is_err());
    }

    #[test]
    fn validate_filters_accepts_known_non_empty_values() {
        let value = validate_filters(&valid_filters()).unwrap();
        assert_eq!(value, json!({ "status": ["active"] }));
    }

    #[test]
    fn validate_filters_rejects_empty_unknown_and_blank_values() {
        assert!(validate_filters(&BTreeMap::new()).is_err());
        assert!(
            validate_filters(&BTreeMap::from([(
                "unknown".to_string(),
                vec!["value".to_string()]
            )]))
            .is_err()
        );
        assert!(validate_filters(&BTreeMap::from([("status".to_string(), Vec::new())])).is_err());
        assert!(
            validate_filters(&BTreeMap::from([(
                "status".to_string(),
                vec!["".to_string()]
            )]))
            .is_err()
        );
    }

    #[test]
    fn validate_filters_rejects_oversized_payload() {
        let filters = BTreeMap::from([(
            "status".to_string(),
            vec!["x".repeat(FILTER_VALUE_MAX_BYTES)],
        )]);
        assert!(validate_filters(&filters).is_err());
    }

    #[test]
    fn response_conversion_rejects_invalid_persisted_filters() {
        let preset = ConnectionFilterPreset {
            id: Uuid::now_v7(),
            name: "bad".to_string(),
            filters: json!("not an object"),
            created_by: Uuid::now_v7(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert!(ConnectionFilterPresetResponse::try_from(preset).is_err());
    }
}
