use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::errors::AppError;

pub const CONTENT_TYPE: &str = "vpn.l2vpn";
pub const VPN_TYPES: &[&str] = &[
    "vpls",
    "vpws",
    "epl",
    "evpl",
    "ep-lan",
    "evp-lan",
    "ep-tree",
    "evp-tree",
    "vxlan",
    "vxlan-evpn",
    "mpls-evpn",
    "pbb-evpn",
    "evpn-vpws",
    "spb",
];
pub const STATUSES: &[&str] = &["active", "planned", "decommissioning"];

pub fn is_point_to_point(vpn_type: &str) -> bool {
    matches!(vpn_type, "vpws" | "epl" | "ep-lan" | "ep-tree")
}

pub fn type_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(VPN_TYPES.iter().copied()))
        .build()
}

pub fn status_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::schema::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(STATUSES.iter().copied()))
        .build()
}

/// Decimal strings preserve the entire signed i64 range for browser consumers.
pub fn parse_identifier(value: &str) -> Result<i64, AppError> {
    let parsed = value.parse::<i64>().map_err(|_| {
        AppError::Validation("identifier must be a canonical signed i64 decimal string".into())
    })?;
    if parsed.to_string() != value {
        return Err(AppError::Validation(
            "identifier must be a canonical signed i64 decimal string".into(),
        ));
    }
    Ok(parsed)
}

fn serialize_identifier<S: serde::Serializer>(
    value: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value.map(|value| value.to_string()).serialize(serializer)
}

#[derive(Debug, Clone, Default, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct L2Vpn {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    #[schema(schema_with = type_schema)]
    pub vpn_type: String,
    #[schema(schema_with = status_schema)]
    pub status: String,
    #[serde(serialize_with = "serialize_identifier")]
    #[schema(value_type = Option<String>, pattern = "^(0|-?[1-9][0-9]*)$")]
    pub identifier: Option<i64>,
    pub tenant_id: Option<Uuid>,
    pub tenant_name: Option<String>,
    pub owner_id: Option<Uuid>,
    pub owner_name: Option<String>,
    pub description: String,
    pub comments: String,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub import_targets: Vec<Uuid>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub export_targets: Vec<Uuid>,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    pub tags: Vec<String>,
    pub custom_fields: serde_json::Value,
    pub created: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
}

impl L2Vpn {
    pub fn validate(&self) -> Result<(), AppError> {
        for targets in [&self.import_targets, &self.export_targets] {
            if targets
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != targets.len()
            {
                return Err(AppError::Validation(
                    "Route target memberships must be unique".into(),
                ));
            }
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::Validation(
                "name must contain 1 to 100 characters".into(),
            ));
        }
        if self.slug.is_empty()
            || self.slug.len() > 100
            || !self
                .slug
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(AppError::Validation(
                "slug must contain 1 to 100 ASCII letters, digits, underscores or hyphens".into(),
            ));
        }
        if !VPN_TYPES.contains(&self.vpn_type.as_str()) {
            return Err(AppError::Validation("unsupported L2VPN type".into()));
        }
        if !STATUSES.contains(&self.status.as_str()) {
            return Err(AppError::Validation("unsupported L2VPN status".into()));
        }
        if self.description.chars().count() > 200 {
            return Err(AppError::Validation(
                "description must be at most 200 characters".into(),
            ));
        }
        if !self.custom_fields.is_object() {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        Ok(())
    }
}
