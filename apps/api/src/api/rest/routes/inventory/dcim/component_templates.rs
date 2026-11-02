//! Native component-template REST routes.
//!
//! Nine typed collections share one permission, merge, validation and
//! transaction path. Each module keeps a concrete OpenAPI contract and its own
//! query surface, so an unsupported filter or body field is rejected instead of
//! being ignored. Authorization of the persisted object and of every supplied
//! parent/reference uses policy attributes read inside the repository
//! transaction, which keeps PAT action and resource ceilings in force.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::InventoryListEnvelope;
use crate::api::middleware::{auth::AuthContext, error::handle_app_error, request_id::RequestId};
use crate::api::permission_helpers::{
    ensure_access_with_resource, ensure_inventory_collection_access,
};
use crate::api::rest::envelope_extractors::{EnvelopeJson, Path, Query, RequireAuthenticated};
use crate::api::rest::request_context::{build_request_context, project_inventory_delete_error};
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::inventory::domain::dcim::component_template::*;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::component_template_repository::{
    COMPONENT_TEMPLATE_COLUMNS, ComponentTemplateAuthorizer, ComponentTemplateKind, ComponentTemplateListFilter,
};
use crate::contexts::shared::list_query::{
    SortSpec, parse_csv_strings, parse_csv_uuids, search_pattern,
};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestContext};
use crate::errors::AppError;

struct LockedAuthorizer<'a> {
    state: &'a AppState,
    context: &'a RequestContext,
}

impl ComponentTemplateAuthorizer for LockedAuthorizer<'_> {
    fn authorize<'a>(
        &'a mut self,
        connection: &'a mut sqlx::PgConnection,
        action: Action,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            let attributes: HashMap<String, Value> = serde_json::from_value(attributes)?;
            let resource = format!("inventory:{id}");
            self.state
                .services
                .policy_evaluation
                .ensure_access_with_attributes_on_connection(
                    connection,
                    self.context,
                    action.clone(),
                    ResourceType::Inventory,
                    Some(&resource),
                    Some(attributes.clone()),
                )
                .await?;
            if self.context.auth_method == "api_token" {
                let token_id = self
                    .context
                    .api_token_id
                    .as_deref()
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or_else(|| AppError::Auth("Invalid API token identity".into()))?;
                let user_id = self.context.require_authenticated_user_id()?;
                let token = self
                    .state
                    .services
                    .user_api_tokens
                    .ensure_current_by_id_on_connection(
                        connection,
                        token_id,
                        user_id,
                        chrono::Utc::now(),
                    )
                    .await?;
                let mut current = self.context.clone();
                current.api_token_scopes = token
                    .scopes
                    .iter()
                    .map(ApiTokenScopeContext::from)
                    .collect();
                current.api_token_scope_resource_override = None;
                self.state
                    .services
                    .policy_evaluation
                    .ensure_access_with_attributes_on_connection(
                        connection,
                        &current,
                        action,
                        ResourceType::Inventory,
                        Some(&resource),
                        Some(attributes),
                    )
                    .await?;
            }
            Ok(())
        })
    }
}

/// Evaluate object access from the attributes persisted with the object.
/// Kept at the API boundary: the repository reads attributes under its lock and
/// this function forwards them to the one policy evaluator, including PAT
/// action and resource ceilings.
async fn authorize_attributes(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Uuid,
    attributes: Value,
) -> Result<(), AppError> {
    let attributes: HashMap<String, Value> = serde_json::from_value(attributes)?;
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

async fn authorize_model<T: serde::Serialize>(
    state: &AppState,
    context: &RequestContext,
    action: Action,
    id: Uuid,
    kind: ComponentTemplateKind,
    item: &T,
) -> Result<(), AppError> {
    let mut attributes = serde_json::to_value(item)?;
    if let Some(object) = attributes.as_object_mut() {
        object.insert("object_type".to_string(), json!(kind.content_type()));
    }
    authorize_attributes(state, context, action, id, attributes).await
}

macro_rules! template_list_query {
    ($name:ident { ids { $($id:ident),* $(,)? }, strings { $($string:ident),* $(,)? } }) => {
        #[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
        #[into_params(parameter_in = Query)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub search: Option<String>,
            pub sort: Option<String>,
            pub order: Option<String>,
            pub limit: Option<u32>,
            pub offset: Option<u32>,
            $(pub $id: Option<String>,)*
            $(pub $string: Option<String>,)*
        }

        impl $name {
            fn filter(&self) -> Result<ComponentTemplateListFilter, AppError> {
                let mut filter = ComponentTemplateListFilter {
                    search: search_pattern(self.search.as_deref()),
                    sort: SortSpec::parse_columns(
                        self.sort.as_deref(),
                        self.order.as_deref(),
                        COMPONENT_TEMPLATE_COLUMNS,
                    )?,
                    ..ComponentTemplateListFilter::default()
                };
                $(
                    filter.$id =
                        parse_csv_uuids(self.$id.as_deref(), stringify!($id))?;
                )*
                $(
                    filter.$string = parse_csv_strings(self.$string.as_deref());
                )*
                Ok(filter)
            }
        }
    };
}

template_list_query!(ConsolePortTemplateListQuery {
    ids { device_type_id, module_type_id },
    strings {}
});
template_list_query!(ConsoleServerPortTemplateListQuery {
    ids { device_type_id, module_type_id },
    strings {}
});
template_list_query!(PowerPortTemplateListQuery {
    ids { device_type_id, module_type_id },
    strings {}
});
template_list_query!(PowerOutletTemplateListQuery {
    ids { device_type_id, module_type_id, power_port_id },
    strings { feed_leg }
});
template_list_query!(InterfaceTemplateListQuery {
    ids { device_type_id, module_type_id, bridge_id },
    strings { enabled, mgmt_only }
});
template_list_query!(FrontPortTemplateListQuery {
    ids { device_type_id, module_type_id },
    strings {}
});
template_list_query!(RearPortTemplateListQuery {
    ids { device_type_id, module_type_id },
    strings {}
});
template_list_query!(DeviceBayTemplateListQuery {
    ids { device_type_id },
    strings { enabled }
});
template_list_query!(InventoryItemTemplateListQuery {
    ids { device_type_id, parent_id, manufacturer_id, role_id },
    strings {}
});

macro_rules! component_template_routes {
    (
        $module:ident,
        $query:ident,
        $model:ident,
        $payload:ident,
        $kind:expr,
        $get:ident,
        $save:ident,
        $collection:literal,
        $object:literal,
        $label:literal,
        $list_op:literal,
        $get_op:literal,
        $create_op:literal,
        $update_op:literal,
        $delete_op:literal
    ) => {
        pub mod $module {
            use super::*;

            #[utoipa::path(get, path=$collection, operation_id=$list_op, tag="inventory", params($query), responses((status=200, body=InventoryListEnvelope<$model>), (status="4XX", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn list(
                State(state): State<AppState>,
                Extension(request_id): Extension<RequestId>,
                auth_context: Option<Extension<AuthContext>>,
                headers: HeaderMap,
                Query(query): Query<$query>,
            ) -> impl IntoResponse {
                let context = build_request_context(&request_id, auth_context, &headers);
                let result = async {
                    ensure_inventory_collection_access(&state, &context, Action::Read).await?;
                    let filter = query.filter()?;
                    let values = state
                        .repositories
                        .component_templates
                        .list($kind, &filter)
                        .await?;
                    // Row-level visibility runs before count and pagination: a
                    // constrained deny that hides an item GET must also hide the
                    // same row from the collection, and `total` must count only
                    // rows the caller may read.
                    let mut visible = Vec::with_capacity(values.len());
                    for value in values {
                        let id = value["id"]
                            .as_str()
                            .and_then(|raw| Uuid::parse_str(raw).ok())
                            .ok_or_else(|| {
                                AppError::Internal(
                                    "Component template row has no parseable id".into(),
                                )
                            })?;
                        match authorize_attributes(
                            &state,
                            &context,
                            Action::Read,
                            id,
                            value.clone(),
                        )
                        .await
                        {
                            Ok(()) => visible.push(value),
                            Err(AppError::Forbidden(_)) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    let total = i64::try_from(visible.len())
                        .map_err(|_| AppError::Internal("Visible row count overflow".into()))?;
                    let limit = clamp_limit(query.limit);
                    let offset = clamp_offset(query.offset);
                    let items = visible
                        .into_iter()
                        .skip(offset as usize)
                        .take(limit as usize)
                        .map(serde_json::from_value::<$model>)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok::<_, AppError>(InventoryListEnvelope {
                        items,
                        total,
                        limit,
                        offset,
                    })
                }
                .await;
                match result {
                    Ok(value) => (StatusCode::OK, Json(value)).into_response(),
                    Err(error) => handle_app_error(error),
                }
            }

            #[utoipa::path(get, path=$object, operation_id=$get_op, tag="inventory", params(("id"=Uuid, Path)), responses((status=200, body=$model), (status="4XX", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn get(
                State(state): State<AppState>,
                Extension(request_id): Extension<RequestId>,
                auth_context: Option<Extension<AuthContext>>,
                headers: HeaderMap,
                Path(id): Path<Uuid>,
            ) -> impl IntoResponse {
                let context = build_request_context(&request_id, auth_context, &headers);
                let result = async {
                    // Existing rows are authorized with their persisted
                    // attributes, so an attribute-scoped policy can grant the
                    // read. A missing row still performs the identity-only
                    // check first, keeping the not-found/forbidden split
                    // without disclosing existence to an anonymous caller.
                    match state.repositories.component_templates.$get(id).await? {
                        Some(item) => {
                            authorize_model(
                                &state,
                                &context,
                                Action::Read,
                                id,
                                $kind,
                                &item,
                            )
                            .await?;
                            Ok::<_, AppError>(item)
                        }
                        None => {
                            ensure_access_with_resource(
                                &state,
                                &context,
                                Action::Read,
                                ResourceType::Inventory,
                                Some(&format!("inventory:{id}")),
                            )
                            .await?;
                            Err(AppError::NotFound(format!("{} not found", $label)))
                        }
                    }
                }
                .await;
                match result {
                    Ok(value) => (StatusCode::OK, Json(value)).into_response(),
                    Err(error) => handle_app_error(error),
                }
            }

            #[utoipa::path(post, path=$collection, operation_id=$create_op, tag="inventory", request_body=$payload, responses((status=201, body=$model), (status="4XX", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn create(
                State(state): State<AppState>,
                Extension(request_id): Extension<RequestId>,
                RequireAuthenticated(auth_context): RequireAuthenticated,
                headers: HeaderMap,
                EnvelopeJson(payload): EnvelopeJson<$payload>,
            ) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    ensure_inventory_collection_access(&state, &context, Action::Create).await?;
                    state
                        .repositories
                        .component_templates
                        .$save(
                            Uuid::now_v7(),
                            payload,
                            true,
                            LockedAuthorizer { state: &state, context: &context },
                        )
                        .await?
                        .ok_or_else(|| {
                            AppError::Internal(format!("Created {} not found", $label))
                        })
                }
                .await;
                match result {
                    Ok(value) => (StatusCode::CREATED, Json(value)).into_response(),
                    Err(error) => handle_app_error(error),
                }
            }

            #[utoipa::path(put, path=$object, operation_id=$update_op, tag="inventory", params(("id"=Uuid, Path)), request_body=$payload, responses((status=200, body=$model), (status="4XX", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn update(
                State(state): State<AppState>,
                Extension(request_id): Extension<RequestId>,
                RequireAuthenticated(auth_context): RequireAuthenticated,
                headers: HeaderMap,
                Path(id): Path<Uuid>,
                EnvelopeJson(payload): EnvelopeJson<$payload>,
            ) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    state
                        .repositories
                        .component_templates
                        .$save(id, payload, false, LockedAuthorizer { state: &state, context: &context })
                        .await?
                        .ok_or_else(|| AppError::NotFound(format!("{} not found", $label)))
                }
                .await;
                match result {
                    Ok(value) => (StatusCode::OK, Json(value)).into_response(),
                    Err(error) => handle_app_error(error),
                }
            }

            #[utoipa::path(delete, path=$object, operation_id=$delete_op, tag="inventory", params(("id"=Uuid, Path)), responses((status=204), (status="4XX", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn delete(
                State(state): State<AppState>,
                Extension(request_id): Extension<RequestId>,
                RequireAuthenticated(auth_context): RequireAuthenticated,
                headers: HeaderMap,
                Path(id): Path<Uuid>,
            ) -> impl IntoResponse {
                let context = build_request_context(&request_id, Some(auth_context), &headers);
                let result = async {
                    state
                        .repositories
                        .component_templates
                        .delete($kind, id, LockedAuthorizer { state: &state, context: &context })
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
        }
    };
}

component_template_routes!(
    console_port_templates,
    ConsolePortTemplateListQuery,
    ConsolePortTemplate,
    ConsolePortTemplateChanges,
    ComponentTemplateKind::ConsolePort,
    get_console_port_template,
    save_console_port_template,
    "/api/v1/inventory/console-port-templates",
    "/api/v1/inventory/console-port-templates/{id}",
    "Console port template",
    "list_console_port_templates",
    "get_console_port_template",
    "create_console_port_template",
    "update_console_port_template",
    "delete_console_port_template"
);

component_template_routes!(
    console_server_port_templates,
    ConsoleServerPortTemplateListQuery,
    ConsoleServerPortTemplate,
    ConsoleServerPortTemplateChanges,
    ComponentTemplateKind::ConsoleServerPort,
    get_console_server_port_template,
    save_console_server_port_template,
    "/api/v1/inventory/console-server-port-templates",
    "/api/v1/inventory/console-server-port-templates/{id}",
    "Console server port template",
    "list_console_server_port_templates",
    "get_console_server_port_template",
    "create_console_server_port_template",
    "update_console_server_port_template",
    "delete_console_server_port_template"
);

component_template_routes!(
    power_port_templates,
    PowerPortTemplateListQuery,
    PowerPortTemplate,
    PowerPortTemplateChanges,
    ComponentTemplateKind::PowerPort,
    get_power_port_template,
    save_power_port_template,
    "/api/v1/inventory/power-port-templates",
    "/api/v1/inventory/power-port-templates/{id}",
    "Power port template",
    "list_power_port_templates",
    "get_power_port_template",
    "create_power_port_template",
    "update_power_port_template",
    "delete_power_port_template"
);

component_template_routes!(
    power_outlet_templates,
    PowerOutletTemplateListQuery,
    PowerOutletTemplate,
    PowerOutletTemplateChanges,
    ComponentTemplateKind::PowerOutlet,
    get_power_outlet_template,
    save_power_outlet_template,
    "/api/v1/inventory/power-outlet-templates",
    "/api/v1/inventory/power-outlet-templates/{id}",
    "Power outlet template",
    "list_power_outlet_templates",
    "get_power_outlet_template",
    "create_power_outlet_template",
    "update_power_outlet_template",
    "delete_power_outlet_template"
);

component_template_routes!(
    interface_templates,
    InterfaceTemplateListQuery,
    InterfaceTemplate,
    InterfaceTemplateChanges,
    ComponentTemplateKind::Interface,
    get_interface_template,
    save_interface_template,
    "/api/v1/inventory/interface-templates",
    "/api/v1/inventory/interface-templates/{id}",
    "Interface template",
    "list_interface_templates",
    "get_interface_template",
    "create_interface_template",
    "update_interface_template",
    "delete_interface_template"
);

component_template_routes!(
    front_port_templates,
    FrontPortTemplateListQuery,
    FrontPortTemplate,
    FrontPortTemplateChanges,
    ComponentTemplateKind::FrontPort,
    get_front_port_template,
    save_front_port_template,
    "/api/v1/inventory/front-port-templates",
    "/api/v1/inventory/front-port-templates/{id}",
    "Front port template",
    "list_front_port_templates",
    "get_front_port_template",
    "create_front_port_template",
    "update_front_port_template",
    "delete_front_port_template"
);

component_template_routes!(
    rear_port_templates,
    RearPortTemplateListQuery,
    RearPortTemplate,
    RearPortTemplateChanges,
    ComponentTemplateKind::RearPort,
    get_rear_port_template,
    save_rear_port_template,
    "/api/v1/inventory/rear-port-templates",
    "/api/v1/inventory/rear-port-templates/{id}",
    "Rear port template",
    "list_rear_port_templates",
    "get_rear_port_template",
    "create_rear_port_template",
    "update_rear_port_template",
    "delete_rear_port_template"
);

component_template_routes!(
    device_bay_templates,
    DeviceBayTemplateListQuery,
    DeviceBayTemplate,
    DeviceBayTemplateChanges,
    ComponentTemplateKind::DeviceBay,
    get_device_bay_template,
    save_device_bay_template,
    "/api/v1/inventory/device-bay-templates",
    "/api/v1/inventory/device-bay-templates/{id}",
    "Device bay template",
    "list_device_bay_templates",
    "get_device_bay_template",
    "create_device_bay_template",
    "update_device_bay_template",
    "delete_device_bay_template"
);

component_template_routes!(
    inventory_item_templates,
    InventoryItemTemplateListQuery,
    InventoryItemTemplate,
    InventoryItemTemplateChanges,
    ComponentTemplateKind::InventoryItem,
    get_inventory_item_template,
    save_inventory_item_template,
    "/api/v1/inventory/inventory-item-templates",
    "/api/v1/inventory/inventory-item-templates/{id}",
    "Inventory item template",
    "list_inventory_item_templates",
    "get_inventory_item_template",
    "create_inventory_item_template",
    "update_inventory_item_template",
    "delete_inventory_item_template"
);
