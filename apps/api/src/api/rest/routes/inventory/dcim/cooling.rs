use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;
use uuid::Uuid;

use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{ensure_access_with_resource, ensure_inventory_collection_access};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::cooling::*;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::cooling_repository::{CoolingFilter, COOLING_COLUMNS};
use crate::contexts::shared::{list_query::{SortSpec, parse_csv_uuids, parse_csv_strings, search_pattern}, pagination::{clamp_limit, clamp_offset}};
use crate::errors::AppError;
use super::super::{InventoryListEnvelope, masked_custom_fields, validate_and_split_custom_fields};

// Per-kind DTOs reject unsupported keys during extraction, even empty values.
// Their filter implementation is shared, as for the native Wireless routes.
macro_rules! cooling_list_query {
    ($name:ident, ids { $($id:ident),* $(,)? }, strings { $($field:ident => $key:literal),* $(,)? }) => {
        #[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
        #[into_params(parameter_in = Query)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub search: Option<String>,
            pub limit: Option<u32>,
            pub offset: Option<u32>,
            pub sort: Option<String>,
            pub order: Option<String>,
            pub id: Option<String>,
            $(pub $id: Option<String>,)*
            $(pub $field: Option<String>,)*
        }

        impl $name {
            fn filter(&self) -> Result<CoolingFilter, AppError> {
                let mut filter = parse_cooling_filters(
                    &[("id", self.id.as_deref()), $((stringify!($id), self.$id.as_deref())),*],
                    &[$(($key, self.$field.as_deref())),*],
                )?;
                filter.search = search_pattern(self.search.as_deref());
                filter.sort = SortSpec::parse_columns(
                    self.sort.as_deref(), self.order.as_deref(), COOLING_COLUMNS,
                )?;
                Ok(filter)
            }
        }
    };
}

cooling_list_query!(CoolingSourceListQuery,
    ids { site_id, location_id }, strings { status => "status", r#type => "type" });
cooling_list_query!(CoolingFeedListQuery,
    ids { cooling_source_id, rack_id, tenant_id }, strings { status => "status" });
cooling_list_query!(CoolingIntakeListQuery,
    ids { device_id, module_id, cooling_outflow_id }, strings { r#type => "type" });
cooling_list_query!(CoolingOutflowListQuery,
    ids { device_id, module_id, cooling_intake_id }, strings { r#type => "type" });
cooling_list_query!(CoolingIntakeTemplateListQuery,
    ids { device_type_id, module_type_id }, strings { r#type => "type" });
cooling_list_query!(CoolingOutflowTemplateListQuery,
    ids { device_type_id, module_type_id, cooling_intake_id }, strings { r#type => "type" });

fn parse_cooling_filters(
    ids: &[(&'static str, Option<&str>)],
    strings: &[(&str, Option<&str>)],
) -> Result<CoolingFilter, AppError> {
    let mut filter = CoolingFilter::default();
    for &(field, raw) in ids {
        if raw.is_some() {
            filter
                .references
                .push((field, parse_csv_uuids(raw, field)?));
        }
    }
    for &(field, raw) in strings {
        let choices = parse_csv_strings(raw);
        match field {
            "status" => filter.status = choices,
            "type" => filter.types = choices,
            _ => {
                return Err(AppError::Internal(
                    "Unknown cooling filter declaration".into(),
                ));
            }
        }
    }
    Ok(filter)
}

pub async fn project(
    state: &AppState,
    kind: CoolingKind,
    mut item: Value,
) -> Result<Value, AppError> {
    if kind.has_custom_fields() {
        let id = item["id"]
            .as_str()
            .ok_or_else(|| AppError::Internal("Missing cooling id".into()))?;
        let id = Uuid::parse_str(id)?;
        if kind == CoolingKind::Source {
            item["coolingfeed_count"] =
                serde_json::json!(state.repositories.cooling.feed_count(id).await?);
        }
        let masked =
            masked_custom_fields(state, kind.content_type(), id, &item["custom_field_data"])
                .await?;
        item.as_object_mut()
            .ok_or_else(|| AppError::Internal("Invalid cooling record".into()))?
            .remove("custom_field_data");
        item["custom_fields"] = masked;
    }
    Ok(item)
}

async fn save(
    state: &AppState,
    kind: CoolingKind,
    id: Uuid,
    mut payload: Value,
    create: bool,
) -> Result<Value, AppError> {
    let mut inputs = Vec::new();
    if kind.has_custom_fields() {
        let fields = payload
            .as_object_mut()
            .ok_or_else(|| AppError::Validation("Expected an object".into()))?
            .remove("custom_fields");
        if !create && fields.as_ref().is_some_and(Value::is_null) {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        let fields = fields.filter(|value| !value.is_null());
        let outcome =
            validate_and_split_custom_fields(state, kind.content_type(), fields, create).await?;
        if let Some(outcome) = outcome {
            payload["custom_field_data"] = outcome.custom_field_data;
            inputs = state
                .services
                .custom_field_values
                .prepare_secret_intents(id, outcome.secret_intents)?;
        }
    }
    let item = state
        .repositories
        .cooling
        .save(kind, id, payload, create, inputs)
        .await?
        .ok_or_else(|| AppError::NotFound("Cooling record not found".into()))?;
    project(state, kind, item).await
}

// Each route has a concrete OpenAPI contract, while all six use the same
// permission, custom-field masking and transactional persistence path.
macro_rules! cooling_routes {
    ($module:ident, $kind:ident, $query:ident, $create:ident, $patch:ident, $model:ident, $collection:literal, $object:literal, $list_id:literal, $get_id:literal, $create_id:literal, $update_id:literal, $delete_id:literal) => {
        pub mod $module {
            use super::*;
            const KIND: CoolingKind = CoolingKind::$kind;

            #[utoipa::path(get, path = $collection, operation_id = $list_id, tag = "inventory", params($query), responses((status = 200, body = InventoryListEnvelope<$model>), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn list(State(state): State<AppState>, Extension(request_id): Extension<RequestId>, auth_context: Option<Extension<AuthContext>>, headers: HeaderMap, Query(query): Query<$query>) -> impl IntoResponse {
                let context = build_request_context(&request_id, auth_context, &headers);
                let result = async {
                    ensure_inventory_collection_access(&state, &context, Action::Read).await?;
                    let (records, total) = state.repositories.cooling.list(KIND, query.filter()?, query.limit, query.offset).await?;
                    let mut items = Vec::with_capacity(records.len());
                    for record in records { items.push(project(&state, KIND, record).await?); }
                    Ok::<_, AppError>(InventoryListEnvelope { items, total, limit: clamp_limit(query.limit), offset: clamp_offset(query.offset) })
                }.await;
                match result { Ok(value) => (StatusCode::OK, Json(value)).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(get, path = $object, operation_id = $get_id, tag = "inventory", params(("id" = Uuid, Path)), responses((status = 200, body = $model), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn get(State(state): State<AppState>, Extension(request_id): Extension<RequestId>, auth_context: Option<Extension<AuthContext>>, headers: HeaderMap, Path(id): Path<Uuid>) -> impl IntoResponse {
                let context = build_request_context(&request_id, auth_context, &headers);
                let result = async {
                    ensure_access_with_resource(&state, &context, Action::Read, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?;
                    let record = state.repositories.cooling.get(KIND, id).await?.ok_or_else(|| AppError::NotFound("Cooling record not found".into()))?;
                    project(&state, KIND, record).await
                }.await;
                match result { Ok(value) => (StatusCode::OK, Json(value)).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(post, path = $collection, operation_id = $create_id, tag = "inventory", request_body = $create, responses((status = 201, body = $model), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn create(State(state): State<AppState>, Extension(request_id): Extension<RequestId>, RequireAuthenticated(auth_context): RequireAuthenticated, headers: HeaderMap, EnvelopeJson(payload): EnvelopeJson<$create>) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    ensure_inventory_collection_access(&state, &context, Action::Create).await?;
                    save(&state, KIND, Uuid::now_v7(), serde_json::to_value(payload)?, true).await
                }.await;
                match result { Ok(value) => (StatusCode::CREATED, Json(value)).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(put, path = $object, operation_id = $update_id, tag = "inventory", params(("id" = Uuid, Path)), request_body = $patch, responses((status = 200, body = $model), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn update(State(state): State<AppState>, Extension(request_id): Extension<RequestId>, RequireAuthenticated(auth_context): RequireAuthenticated, headers: HeaderMap, Path(id): Path<Uuid>, EnvelopeJson(payload): EnvelopeJson<$patch>) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    ensure_access_with_resource(&state, &context, Action::Update, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?;
                    save(&state, KIND, id, serde_json::to_value(payload)?, false).await
                }.await;
                match result { Ok(value) => (StatusCode::OK, Json(value)).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(delete, path = $object, operation_id = $delete_id, tag = "inventory", params(("id" = Uuid, Path)), responses((status = 204), (status = "4XX", body = crate::api::rest::error_mapping::ApiErrorResponse), (status = "5XX", body = crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn delete(State(state): State<AppState>, Extension(request_id): Extension<RequestId>, RequireAuthenticated(auth_context): RequireAuthenticated, headers: HeaderMap, Path(id): Path<Uuid>) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    ensure_access_with_resource(&state, &context, Action::Delete, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?;
                    state.repositories.cooling.delete(KIND, id).await
                }.await;
                match result { Ok(()) => StatusCode::NO_CONTENT.into_response(), Err(error) => handle_app_error(project_inventory_delete_error(&state, &context, error).await) }
            }
        }
    };
}

cooling_routes!(
    sources,
    Source,
    CoolingSourceListQuery,
    CoolingSourceCreate,
    CoolingSourcePatch,
    CoolingSource,
    "/api/v1/inventory/cooling-sources",
    "/api/v1/inventory/cooling-sources/{id}",
    "list_cooling_sources",
    "get_cooling_source",
    "create_cooling_source",
    "update_cooling_source",
    "delete_cooling_source"
);
cooling_routes!(
    feeds,
    Feed,
    CoolingFeedListQuery,
    CoolingFeedCreate,
    CoolingFeedPatch,
    CoolingFeed,
    "/api/v1/inventory/cooling-feeds",
    "/api/v1/inventory/cooling-feeds/{id}",
    "list_cooling_feeds",
    "get_cooling_feed",
    "create_cooling_feed",
    "update_cooling_feed",
    "delete_cooling_feed"
);
cooling_routes!(
    intakes,
    Intake,
    CoolingIntakeListQuery,
    CoolingIntakeCreate,
    CoolingIntakePatch,
    CoolingIntake,
    "/api/v1/inventory/cooling-intakes",
    "/api/v1/inventory/cooling-intakes/{id}",
    "list_cooling_intakes",
    "get_cooling_intake",
    "create_cooling_intake",
    "update_cooling_intake",
    "delete_cooling_intake"
);
cooling_routes!(
    outflows,
    Outflow,
    CoolingOutflowListQuery,
    CoolingOutflowCreate,
    CoolingOutflowPatch,
    CoolingOutflow,
    "/api/v1/inventory/cooling-outflows",
    "/api/v1/inventory/cooling-outflows/{id}",
    "list_cooling_outflows",
    "get_cooling_outflow",
    "create_cooling_outflow",
    "update_cooling_outflow",
    "delete_cooling_outflow"
);
cooling_routes!(
    intake_templates,
    IntakeTemplate,
    CoolingIntakeTemplateListQuery,
    CoolingIntakeTemplateCreate,
    CoolingIntakeTemplatePatch,
    CoolingIntakeTemplate,
    "/api/v1/inventory/cooling-intake-templates",
    "/api/v1/inventory/cooling-intake-templates/{id}",
    "list_cooling_intake_templates",
    "get_cooling_intake_template",
    "create_cooling_intake_template",
    "update_cooling_intake_template",
    "delete_cooling_intake_template"
);
cooling_routes!(
    outflow_templates,
    OutflowTemplate,
    CoolingOutflowTemplateListQuery,
    CoolingOutflowTemplateCreate,
    CoolingOutflowTemplatePatch,
    CoolingOutflowTemplate,
    "/api/v1/inventory/cooling-outflow-templates",
    "/api/v1/inventory/cooling-outflow-templates/{id}",
    "list_cooling_outflow_templates",
    "get_cooling_outflow_template",
    "create_cooling_outflow_template",
    "update_cooling_outflow_template",
    "delete_cooling_outflow_template"
);
