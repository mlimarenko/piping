use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

/// Write-only mutation of this group's managed authentication credential.
/// Omission preserves the binding; clearing requires an explicit intent.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FhrpAuthKeyInput {
    pub intent: crate::contexts::secrets::application::SecretFieldIntent,
    pub value: Option<String>,
}

/// Request boundary for group writes. Serde delegates to the business model,
/// while the request schema includes the credential intent omitted from reads.
#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct FhrpGroupRequest(pub FhrpGroup);

impl utoipa::PartialSchema for FhrpGroupRequest {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::{RefOr, schema::Schema};

        // utoipa-gen 5.5 filters skip_serializing fields even with write_only.
        // Extend the same group's object schema rather than duplicating its
        // metadata or changing its fail-closed serialization policy.
        let RefOr::T(Schema::Object(mut schema)) = <FhrpGroup as utoipa::PartialSchema>::schema()
        else {
            unreachable!("FhrpGroup derives an object schema");
        };
        schema.properties.insert(
            "auth_key".into(),
            <Option<FhrpAuthKeyInput> as utoipa::PartialSchema>::schema(),
        );
        schema.properties.insert(
            "tags".into(),
            crate::contexts::inventory::domain::extras::tag::membership_schema().into(),
        );
        RefOr::T(Schema::Object(schema))
    }
}

impl utoipa::ToSchema for FhrpGroupRequest {
    fn schemas(
        schemas: &mut Vec<(
            String,
            utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
        )>,
    ) {
        <FhrpGroup as utoipa::ToSchema>::schemas(schemas);
        <FhrpAuthKeyInput as utoipa::ToSchema>::schemas(schemas);
    }
}

/// A first-hop redundancy protocol group and its persisted inventory metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FhrpGroup {
    pub id: Uuid,
    pub group_id: i16,
    pub name: String,
    pub protocol: String,
    pub auth_type: Option<String>,
    #[serde(skip_serializing)]
    pub auth_key: Option<FhrpAuthKeyInput>,
    pub description: String,
    pub comments: String,
    pub owner_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_fields: Option<serde_json::Value>,
    /// Server-managed creation timestamp; ignored on write.
    #[schema(read_only)]
    pub created: Option<DateTime<Utc>>,
    /// Server-managed update timestamp; ignored on write.
    #[schema(read_only)]
    pub last_updated: Option<DateTime<Utc>>,
    pub external_id: Option<i64>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

impl FhrpGroup {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.group_id < 0 {
            return Err(AppError::Validation("group_id must be non-negative".into()));
        }
        if !matches!(
            self.protocol.as_str(),
            "vrrp2" | "vrrp3" | "carp" | "clusterxl" | "glbp" | "hsrp" | "other"
        ) {
            return Err(AppError::Validation("Unsupported FHRP protocol".into()));
        }
        if !matches!(
            self.auth_type.as_deref(),
            None | Some("" | "plaintext" | "md5")
        ) {
            return Err(AppError::Validation(
                "Unsupported FHRP authentication type".into(),
            ));
        }
        if self.name.chars().count() > 100
            || self.description.chars().count() > 200
            || self
                .auth_key
                .as_ref()
                .and_then(|key| key.value.as_ref())
                .is_some_and(|value| value.chars().count() > 255)
        {
            return Err(AppError::Validation(
                "FHRP name, description or authentication key exceeds its storage limit".into(),
            ));
        }
        if self
            .custom_fields
            .as_ref()
            .is_some_and(|value| !value.is_object())
        {
            return Err(AppError::Validation(
                "custom_fields must be an object".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_protocol_authentication_and_storage_boundaries() {
        let mut group = FhrpGroup {
            protocol: "vrrp3".into(),
            ..Default::default()
        };
        assert!(group.validate().is_ok());
        group.group_id = -1;
        assert!(group.validate().is_err());
        group.group_id = i16::MAX;
        group.auth_type = Some("md5".into());
        group.name = "я".repeat(100);
        assert!(group.validate().is_ok());
        group.name.push('x');
        assert!(group.validate().is_err());
        group.name.clear();
        group.auth_type = Some("invalid".into());
        assert!(group.validate().is_err());
        group.auth_type = None;
        group.protocol = "invalid".into();
        assert!(group.validate().is_err());
    }
}
