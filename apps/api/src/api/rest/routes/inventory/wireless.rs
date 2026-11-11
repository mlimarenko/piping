//! Wireless CRUD and bulk creation use the same native permission and metadata owners.

use super::{
    BulkCreateRequest, BulkCreateResponse, InventoryListEnvelope, ensure_create_inventory_access,
    masked_custom_fields, masked_custom_fields_page, parse_uuid, push_bulk_error,
    require_bulk_items, validate_and_split_custom_fields,
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
use crate::contexts::inventory::domain::wireless::*;
use crate::contexts::inventory::infrastructure::persistence::repositories::wireless::{
    WirelessKind, WirelessListFilter,
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

const WIRELESS_AUTH_TYPES: &[&str] = &["open", "wep", "wpa-personal", "wpa-enterprise"];
const WIRELESS_AUTH_CIPHERS: &[&str] = &["auto", "tkip", "aes"];

macro_rules! wireless_list_query {
    ($name:ident, $kind:ident, ids { $($id:ident),* $(,)? }, strings { $($field:ident => $allowed:expr),* $(,)? }) => {
        #[derive(Debug, serde::Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        #[into_params(parameter_in = Query)]
        pub struct $name {
            pub limit: Option<u32>,
            pub offset: Option<u32>,
            pub search: Option<String>,
            pub sort: Option<String>,
            pub order: Option<String>,
            $(pub $id: Option<String>,)*
            $(pub $field: Option<String>,)*
        }

        impl $name {
            fn filter(&self) -> Result<WirelessListFilter, AppError> {
                parse_wireless_filters(
                    WirelessKind::$kind,
                    &[$((stringify!($id), self.$id.as_deref())),*],
                    &[$((stringify!($field), self.$field.as_deref(), $allowed)),*],
                )
            }
        }
    };
}

wireless_list_query!(
    WirelessGroupListQuery,
    Group,
    ids {
        parent_id,
        owner_id
    },
    strings {}
);
wireless_list_query!(WirelessLanListQuery, Lan,
    ids { group_id, vlan_id, tenant_id, owner_id, scope_id },
    strings {
        scope_type => &["dcim.region", "dcim.sitegroup", "dcim.site", "dcim.location"],
        status => &["active", "reserved", "disabled", "deprecated"],
        auth_type => WIRELESS_AUTH_TYPES,
        auth_cipher => WIRELESS_AUTH_CIPHERS,
    }
);
wireless_list_query!(WirelessLinkListQuery, Link,
    ids { interface_a_id, interface_b_id, tenant_id, owner_id },
    strings {
        status => &["connected", "planned", "decommissioning"],
        auth_type => WIRELESS_AUTH_TYPES,
        auth_cipher => WIRELESS_AUTH_CIPHERS,
        distance_unit => &["m", "km", "ft", "mi"],
    }
);

fn parse_wireless_filters(
    kind: WirelessKind,
    ids: &[(&'static str, Option<&str>)],
    strings: &[(&'static str, Option<&str>, &[&str])],
) -> Result<WirelessListFilter, AppError> {
    let mut filter = WirelessListFilter::default();
    for &(param, raw) in ids {
        if raw.is_some() {
            if !kind
                .columns()
                .iter()
                .any(|column| column.filter.is_some_and(|f| f.param == param))
            {
                return Err(AppError::Validation(format!(
                    "Unsupported Wireless filter: {param}"
                )));
            }
            filter.ids.push((param, parse_csv_uuids(raw, param)?));
        }
    }
    for &(param, raw, allowed) in strings {
        if raw.is_some() {
            let values = parse_csv_strings(raw);
            if !kind
                .columns()
                .iter()
                .any(|column| column.filter.is_some_and(|f| f.param == param))
                || values
                    .iter()
                    .any(|value| !allowed.contains(&value.as_str()))
            {
                return Err(AppError::Validation(format!(
                    "Invalid Wireless filter: {param}"
                )));
            }
            filter.strings.push((param, values));
        }
    }
    if filter
        .ids
        .iter()
        .any(|(param, ids)| *param == "scope_id" && !ids.is_empty())
        && !filter
            .strings
            .iter()
            .any(|(param, values)| *param == "scope_type" && !values.is_empty())
    {
        return Err(AppError::Validation("scope_id requires scope_type".into()));
    }
    Ok(filter)
}

async fn authorize_references(
    state: &AppState,
    ctx: &RequestContext,
    ids: Vec<Uuid>,
) -> Result<(), AppError> {
    for id in ids {
        ensure_access_with_resource(
            state,
            ctx,
            Action::Read,
            ResourceType::Inventory,
            Some(&format!("inventory:{id}")),
        )
        .await?;
    }
    Ok(())
}

macro_rules! wireless_routes {
    ($module:ident, $model:ident, $create:ident, $write:ident, $kind:ident, $query:ident, $content:literal,
     $collection:literal, $detail:literal, $bulk:literal,
     $list_repo:ident, $get_repo:ident, $create_repo:ident, $update_repo:ident,
     $list_op:literal, $get_op:literal, $create_op:literal, $bulk_op:literal, $put_op:literal, $patch_op:literal, $delete_op:literal) => {
        pub mod $module {
            use super::*;

            async fn mask(state: &AppState, mut value: $model) -> Result<$model, AppError> {
                value.custom_fields = masked_custom_fields(state, $content, value.id, &value.custom_fields).await?;
                Ok(value)
            }

            async fn create_record(state: &AppState, ctx: &RequestContext, mut payload: $write) -> Result<$model, AppError> {
                authorize_references(state, ctx, payload.reference_ids()).await?;
                let outcome = validate_and_split_custom_fields(state, $content, payload.custom_fields.clone(), true).await?;
                let intents = if let Some(outcome) = outcome { payload.custom_fields = Some(outcome.custom_field_data); outcome.secret_intents } else { Vec::new() };
                let value = payload.create()?;
                let inputs = state.services.custom_field_values.prepare_secret_intents(value.id, intents)?;
                let value = state.repositories.wireless.$create_repo(value, inputs).await?;
                mask(state, value).await
            }

            #[utoipa::path(get, path=$collection, operation_id=$list_op, tag="inventory", params($query), responses((status=200, description="Wireless collection", body=InventoryListEnvelope<$model>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn list(State(state): State<AppState>, Extension(rid): Extension<RequestId>, auth: Option<Extension<AuthContext>>, headers: HeaderMap, Query(query): Query<$query>) -> Response {
                let ctx = build_request_context(&rid, auth, &headers);
                let result = async {
                    ensure_inventory_collection_access(&state, &ctx, Action::Read).await?;
                    let sort = SortSpec::parse_columns(query.sort.as_deref(), query.order.as_deref(), WirelessKind::$kind.columns())?;
                    let mut filter = query.filter()?;
                    filter.search = search_pattern(query.search.as_deref());
                    filter.sort = sort;
                    let (mut items, total) = state.repositories.wireless.$list_repo(query.limit, query.offset, filter).await?;
                    let fields = masked_custom_fields_page(&state, $content, items.iter().map(|v| (v.id, v.custom_fields.clone())).collect()).await?;
                    for (value, fields) in items.iter_mut().zip(fields) { value.custom_fields = fields; }
                    Ok::<_, AppError>(InventoryListEnvelope { items, total, limit: clamp_limit(query.limit), offset: clamp_offset(query.offset) })
                }.await;
                match result { Ok(value) => Json(value).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(get, path=$detail, operation_id=$get_op, tag="inventory", params(("id"=Uuid, Path)), responses((status=200, description="Wireless object", body=$model), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn get(State(state): State<AppState>, Extension(rid): Extension<RequestId>, auth: Option<Extension<AuthContext>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
                let ctx = build_request_context(&rid, auth, &headers);
                let result = async {
                    let id = parse_uuid(&id, "id")?;
                    ensure_access_with_resource(&state, &ctx, Action::Read, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?;
                    let value = state.repositories.wireless.$get_repo(id).await?.ok_or_else(|| AppError::NotFound("Wireless object not found".into()))?;
                    mask(&state, value).await
                }.await;
                match result { Ok(value) => Json(value).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(post, path=$collection, operation_id=$create_op, tag="inventory", request_body=$create, responses((status=201, description="Created", body=$model), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn create(State(state): State<AppState>, Extension(rid): Extension<RequestId>, RequireAuthenticated(auth): RequireAuthenticated, headers: HeaderMap, EnvelopeJson(payload): EnvelopeJson<$create>) -> Response {
                let ctx = build_request_context(&rid, Some(auth), &headers);
                if let Err(error) = ensure_create_inventory_access(&state, &ctx).await { return handle_app_error(error); }
                match create_record(&state, &ctx, payload.into()).await { Ok(value) => (StatusCode::CREATED, Json(value)).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(post, path=$bulk, operation_id=$bulk_op, tag="inventory", request_body=BulkCreateRequest<$create>, responses((status=200, description="Bulk result", body=BulkCreateResponse<$model>), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn bulk_create(State(state): State<AppState>, Extension(rid): Extension<RequestId>, RequireAuthenticated(auth): RequireAuthenticated, headers: HeaderMap, EnvelopeJson(payload): EnvelopeJson<BulkCreateRequest<$create>>) -> Response {
                let ctx = build_request_context(&rid, Some(auth), &headers);
                if let Err(error) = ensure_create_inventory_access(&state, &ctx).await { return handle_app_error(error); }
                let items = match require_bulk_items(payload.items) { Ok(items) => items, Err(error) => return handle_app_error(error) };
                let mut created = Vec::new(); let mut errors = Vec::new();
                for (index, item) in items.into_iter().enumerate() { match create_record(&state, &ctx, item.into()).await { Ok(value) => created.push(value), Err(error) => push_bulk_error(&mut errors, index, error) } }
                Json(BulkCreateResponse { created, errors }).into_response()
            }

            async fn write(state: AppState, ctx: RequestContext, id: String, mut payload: $write) -> Response {
                let result = async {
                    let id = parse_uuid(&id, "id")?;
                    ensure_access_with_resource(&state, &ctx, Action::Update, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?;
                    authorize_references(&state, &ctx, payload.reference_ids()).await?;
                    let outcome = validate_and_split_custom_fields(&state, $content, payload.custom_fields.clone(), false).await?;
                    let intents = if let Some(outcome) = outcome { payload.custom_fields = Some(outcome.custom_field_data); outcome.secret_intents } else { Vec::new() };
                    let inputs = state.services.custom_field_values.prepare_secret_intents(id, intents)?;
                    let value = state.repositories.wireless.$update_repo(id, payload, inputs).await?.ok_or_else(|| AppError::NotFound("Wireless object not found".into()))?;
                    mask(&state, value).await
                }.await;
                match result { Ok(value) => Json(value).into_response(), Err(error) => handle_app_error(error) }
            }

            #[utoipa::path(put, path=$detail, operation_id=$put_op, tag="inventory", params(("id"=Uuid, Path)), request_body=$write, responses((status=200, description="Updated; omitted fields retained", body=$model), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn update(State(state): State<AppState>, Extension(rid): Extension<RequestId>, RequireAuthenticated(auth): RequireAuthenticated, headers: HeaderMap, Path(id): Path<String>, EnvelopeJson(payload): EnvelopeJson<$write>) -> Response { let ctx = build_request_context(&rid, Some(auth), &headers); write(state, ctx, id, payload).await }

            #[utoipa::path(patch, path=$detail, operation_id=$patch_op, tag="inventory", params(("id"=Uuid, Path)), request_body=$write, responses((status=200, description="Patched", body=$model), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn patch(State(state): State<AppState>, Extension(rid): Extension<RequestId>, RequireAuthenticated(auth): RequireAuthenticated, headers: HeaderMap, Path(id): Path<String>, EnvelopeJson(payload): EnvelopeJson<$write>) -> Response { let ctx = build_request_context(&rid, Some(auth), &headers); write(state, ctx, id, payload).await }

            #[utoipa::path(delete, path=$detail, operation_id=$delete_op, tag="inventory", params(("id"=Uuid, Path)), responses((status=204, description="Deleted"), (status="4XX", description="Rejected", body=crate::api::rest::error_mapping::ApiErrorResponse), (status="5XX", description="Failed", body=crate::api::rest::error_mapping::ApiErrorResponse)))]
            pub async fn delete(State(state): State<AppState>, Extension(rid): Extension<RequestId>, RequireAuthenticated(auth): RequireAuthenticated, headers: HeaderMap, Path(id): Path<String>) -> Response {
                let ctx = build_request_context(&rid, Some(auth), &headers);
                let result = async { let id = parse_uuid(&id, "id")?; ensure_access_with_resource(&state, &ctx, Action::Delete, ResourceType::Inventory, Some(&format!("inventory:{id}"))).await?; state.repositories.wireless.delete(WirelessKind::$kind, id).await }.await;
                match result { Ok(()) => StatusCode::NO_CONTENT.into_response(), Err(error) => handle_app_error(error) }
            }
        }
    };
}

wireless_routes!(
    groups,
    WirelessLanGroup,
    WirelessLanGroupCreate,
    WirelessLanGroupWrite,
    Group,
    WirelessGroupListQuery,
    "wireless.wirelesslangroup",
    "/api/v1/inventory/wireless_lan_groups",
    "/api/v1/inventory/wireless_lan_groups/{id}",
    "/api/v1/inventory/wireless_lan_groups/bulk",
    list_groups,
    get_group,
    create_group,
    update_group,
    "list_wireless_lan_groups",
    "get_wireless_lan_group",
    "create_wireless_lan_group",
    "bulk_create_wireless_lan_groups",
    "update_wireless_lan_group",
    "patch_wireless_lan_group",
    "delete_wireless_lan_group"
);
wireless_routes!(
    lans,
    WirelessLan,
    WirelessLanCreate,
    WirelessLanWrite,
    Lan,
    WirelessLanListQuery,
    "wireless.wirelesslan",
    "/api/v1/inventory/wireless_lans",
    "/api/v1/inventory/wireless_lans/{id}",
    "/api/v1/inventory/wireless_lans/bulk",
    list_lans,
    get_lan,
    create_lan,
    update_lan,
    "list_wireless_lans",
    "get_wireless_lan",
    "create_wireless_lan",
    "bulk_create_wireless_lans",
    "update_wireless_lan",
    "patch_wireless_lan",
    "delete_wireless_lan"
);
wireless_routes!(
    links,
    WirelessLink,
    WirelessLinkCreate,
    WirelessLinkWrite,
    Link,
    WirelessLinkListQuery,
    "wireless.wirelesslink",
    "/api/v1/inventory/wireless_links",
    "/api/v1/inventory/wireless_links/{id}",
    "/api/v1/inventory/wireless_links/bulk",
    list_links,
    get_link,
    create_link,
    update_link,
    "list_wireless_links",
    "get_wireless_link",
    "create_wireless_link",
    "bulk_create_wireless_links",
    "update_wireless_link",
    "patch_wireless_link",
    "delete_wireless_link"
);
