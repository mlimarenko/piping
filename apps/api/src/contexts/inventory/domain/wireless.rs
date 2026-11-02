//! Wireless inventory records and validation shared by native writes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

macro_rules! wireless_record {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
        pub struct $name {
            pub id: Uuid,
            $(pub $field: $ty,)*
            pub description: String,
            pub comments: String,
            pub owner_id: Option<Uuid>,
            pub tags: Vec<String>,
            pub custom_fields: serde_json::Value,
            pub created: Option<DateTime<Utc>>,
            pub last_updated: Option<DateTime<Utc>>,
            pub external_id: Option<i64>,
            pub piping_synced_at: Option<DateTime<Utc>>,
        }
    };
}

wireless_record!(WirelessLanGroup {
    name: String,
    slug: String,
    parent_id: Option<Uuid>,
});

wireless_record!(WirelessLan {
    ssid: String,
    group_id: Option<Uuid>,
    status: String,
    vlan_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    scope_type: Option<String>,
    scope_id: Option<Uuid>,
    auth_type: Option<String>,
    auth_cipher: Option<String>,
    auth_psk: String,
});

wireless_record!(WirelessLink {
    interface_a_id: Uuid,
    interface_b_id: Uuid,
    ssid: String,
    status: String,
    tenant_id: Option<Uuid>,
    auth_type: Option<String>,
    auth_cipher: Option<String>,
    auth_psk: String,
    distance: Option<f64>,
    distance_unit: Option<String>,
});

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

macro_rules! wireless_write {
    ($name:ident, $record:ident, required { $($field:ident: $ty:ty),* $(,)? }, nullable { $($optional:ident: $optional_ty:ty),* $(,)? }) => {
        /// Sparse wireless command. Omitted fields are retained; null clears nullable fields.
        #[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(#[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub $field: Option<$ty>,)*
            $(#[serde(default, deserialize_with = "nullable")]
            pub $optional: Option<Option<$optional_ty>>,)*
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub description: Option<String>,
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub comments: Option<String>,
            #[serde(default, deserialize_with = "nullable")]
            pub owner_id: Option<Option<Uuid>>,
            #[serde(default, deserialize_with = "present")]
            #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
            pub tags: Option<Vec<String>>,
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub custom_fields: Option<serde_json::Value>,
        }

        impl $name {
            pub fn apply(self, record: &mut $record) -> Result<(), AppError> {
                $(if let Some(value) = self.$field { record.$field = value; })*
                $(if let Some(value) = self.$optional { record.$optional = value; })*
                if let Some(value) = self.description { record.description = value; }
                if let Some(value) = self.comments { record.comments = value; }
                if let Some(value) = self.owner_id { record.owner_id = value; }
                if let Some(value) = self.tags { record.tags = value; }
                if let Some(value) = self.custom_fields { record.custom_fields = value; }
                record.validate()
            }
        }
    };
}

wireless_write!(
    WirelessLanGroupWrite,
    WirelessLanGroup,
    required {
        name: String,
        slug: String
    },
    nullable { parent_id: Uuid }
);
wireless_write!(
    WirelessLanWrite,
    WirelessLan,
    required {
        ssid: String,
        status: String,
        auth_psk: String
    },
    nullable {
        group_id: Uuid,
        vlan_id: Uuid,
        tenant_id: Uuid,
        scope_type: String,
        scope_id: Uuid,
        auth_type: String,
        auth_cipher: String
    }
);
wireless_write!(
    WirelessLinkWrite,
    WirelessLink,
    required {
        interface_a_id: Uuid,
        interface_b_id: Uuid,
        ssid: String,
        status: String,
        auth_psk: String
    },
    nullable {
        tenant_id: Uuid,
        auth_type: String,
        auth_cipher: String,
        distance: f64,
        distance_unit: String
    }
);

macro_rules! wireless_create {
    ($name:ident, $write:ident, required { $($required:ident: $required_ty:ty),* $(,)? }, values { $($field:ident: $ty:ty),* $(,)? }, nullable { $($optional:ident: $optional_ty:ty),* $(,)? }) => {
        /// Create input with required identifiers and no server-managed metadata.
        #[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(pub $required: $required_ty,)*
            $(#[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub $field: Option<$ty>,)*
            $(pub $optional: Option<$optional_ty>,)*
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub description: Option<String>,
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub comments: Option<String>,
            pub owner_id: Option<Uuid>,
            #[serde(default, deserialize_with = "present")]
            #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
            pub tags: Option<Vec<String>>,
            #[serde(default, deserialize_with = "present")]
            #[schema(nullable = false)]
            pub custom_fields: Option<serde_json::Value>,
        }
        impl From<$name> for $write {
            fn from(value: $name) -> Self {
                Self {
                    $($required: Some(value.$required),)*
                    $($field: value.$field,)*
                    $($optional: Some(value.$optional),)*
                    description: value.description, comments: value.comments,
                    owner_id: Some(value.owner_id), tags: value.tags,
                    custom_fields: value.custom_fields,
                }
            }
        }
    };
}
wireless_create!(
    WirelessLanGroupCreate,
    WirelessLanGroupWrite,
    required {
        name: String,
        slug: String
    },
    values {},
    nullable { parent_id: Uuid }
);
wireless_create!(
    WirelessLanCreate,
    WirelessLanWrite,
    required { ssid: String },
    values {
        status: String,
        auth_psk: String
    },
    nullable {
        group_id: Uuid,
        vlan_id: Uuid,
        tenant_id: Uuid,
        scope_type: String,
        scope_id: Uuid,
        auth_type: String,
        auth_cipher: String
    }
);
wireless_create!(
    WirelessLinkCreate,
    WirelessLinkWrite,
    required {
        interface_a_id: Uuid,
        interface_b_id: Uuid
    },
    values {
        ssid: String,
        status: String,
        auth_psk: String
    },
    nullable {
        tenant_id: Uuid,
        auth_type: String,
        auth_cipher: String,
        distance: f64,
        distance_unit: String
    }
);

impl WirelessLanGroupWrite {
    pub fn reference_ids(&self) -> Vec<Uuid> {
        [self.parent_id.flatten(), self.owner_id.flatten()]
            .into_iter()
            .flatten()
            .collect()
    }
    pub fn create(self) -> Result<WirelessLanGroup, AppError> {
        let mut value = WirelessLanGroup {
            id: Uuid::now_v7(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        self.apply(&mut value)?;
        Ok(value)
    }
}

impl WirelessLanWrite {
    pub fn reference_ids(&self) -> Vec<Uuid> {
        [
            self.group_id.flatten(),
            self.vlan_id.flatten(),
            self.tenant_id.flatten(),
            self.scope_id.flatten(),
            self.owner_id.flatten(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
    pub fn create(self) -> Result<WirelessLan, AppError> {
        let mut value = WirelessLan {
            id: Uuid::now_v7(),
            status: "active".into(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        self.apply(&mut value)?;
        Ok(value)
    }
}

impl WirelessLinkWrite {
    pub fn reference_ids(&self) -> Vec<Uuid> {
        [
            self.interface_a_id,
            self.interface_b_id,
            self.tenant_id.flatten(),
            self.owner_id.flatten(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
    pub fn create(self) -> Result<WirelessLink, AppError> {
        let mut value = WirelessLink {
            id: Uuid::now_v7(),
            status: "connected".into(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        self.apply(&mut value)?;
        Ok(value)
    }
}

fn length(value: &str, field: &str, minimum: usize, maximum: usize) -> Result<(), AppError> {
    let len = value.chars().count();
    if len < minimum || len > maximum || (minimum > 0 && value.trim().is_empty()) {
        return Err(AppError::Validation(format!(
            "{field} must contain {minimum}–{maximum} characters"
        )));
    }
    Ok(())
}

fn choice(value: &str, field: &str, choices: &[&str]) -> Result<(), AppError> {
    if !choices.contains(&value) {
        return Err(AppError::Validation(format!("Invalid {field}: {value}")));
    }
    Ok(())
}

fn auth(auth_type: Option<&str>, cipher: Option<&str>, psk: &str) -> Result<(), AppError> {
    if let Some(value) = auth_type {
        choice(
            value,
            "auth_type",
            &["open", "wep", "wpa-personal", "wpa-enterprise"],
        )?;
    }
    if let Some(value) = cipher {
        choice(value, "auth_cipher", &["auto", "tkip", "aes"])?;
    }
    // Never include the supplied credential in a validation error.
    length(psk, "auth_psk", 0, 64)
}

fn metadata(description: &str, custom_fields: &serde_json::Value) -> Result<(), AppError> {
    length(description, "description", 0, 200)?;
    if !custom_fields.is_object() {
        return Err(AppError::Validation(
            "custom_fields must be an object".into(),
        ));
    }
    Ok(())
}

impl WirelessLanGroup {
    pub fn validate(&self) -> Result<(), AppError> {
        length(&self.name, "name", 1, 100)?;
        length(&self.slug, "slug", 1, 100)?;
        if !self
            .slug
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(AppError::Validation(
                "slug may contain only letters, numbers, underscores and hyphens".into(),
            ));
        }
        if self.parent_id == Some(self.id) {
            return Err(AppError::Validation(
                "parent_id cannot reference the same wireless LAN group".into(),
            ));
        }
        metadata(&self.description, &self.custom_fields)
    }
}

impl WirelessLan {
    pub fn validate(&self) -> Result<(), AppError> {
        length(&self.ssid, "ssid", 1, 32)?;
        choice(
            &self.status,
            "status",
            &["active", "reserved", "disabled", "deprecated"],
        )?;
        if self.scope_type.is_some() != self.scope_id.is_some() {
            return Err(AppError::Validation(
                "scope_type and scope_id must be set or cleared together".into(),
            ));
        }
        if let Some(scope) = self.scope_type.as_deref() {
            choice(
                scope,
                "scope_type",
                &[
                    "dcim.region",
                    "dcim.sitegroup",
                    "dcim.site",
                    "dcim.location",
                ],
            )?;
        }
        auth(
            self.auth_type.as_deref(),
            self.auth_cipher.as_deref(),
            &self.auth_psk,
        )?;
        metadata(&self.description, &self.custom_fields)
    }
}

impl WirelessLink {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.interface_a_id.is_nil() || self.interface_b_id.is_nil() {
            return Err(AppError::Validation(
                "interface_a_id and interface_b_id are required".into(),
            ));
        }
        if self.interface_a_id == self.interface_b_id {
            return Err(AppError::Validation(
                "Wireless link endpoints must be different interfaces".into(),
            ));
        }
        length(&self.ssid, "ssid", 0, 32)?;
        choice(
            &self.status,
            "status",
            &["connected", "planned", "decommissioning"],
        )?;
        if let Some(distance) = self.distance {
            if !distance.is_finite()
                || distance < 0.0
                || distance > 999_999.99
                || distance.to_bits() != ((distance * 100.0).round() / 100.0).to_bits()
            {
                return Err(AppError::Validation(
                    "distance must be between 0 and 999999.99 with at most two decimal places"
                        .into(),
                ));
            }
            if self.distance_unit.is_none() {
                return Err(AppError::Validation(
                    "distance_unit is required with distance".into(),
                ));
            }
        }
        if let Some(unit) = self.distance_unit.as_deref() {
            choice(unit, "distance_unit", &["m", "km", "ft", "mi"])?;
        }
        auth(
            self.auth_type.as_deref(),
            self.auth_cipher.as_deref(),
            &self.auth_psk,
        )?;
        metadata(&self.description, &self.custom_fields)
    }
}

/// NetBox 4.7.1 wireless interface choices, including PAN and cellular links.
#[must_use]
pub fn is_wireless_interface(interface_type: &str) -> bool {
    matches!(
        interface_type,
        "ieee802.11a"
            | "ieee802.11g"
            | "ieee802.11n"
            | "ieee802.11ac"
            | "ieee802.11ad"
            | "ieee802.11ax"
            | "ieee802.11ay"
            | "ieee802.11be"
            | "ieee802.15.1"
            | "ieee802.15.4"
            | "other-wireless"
            | "gsm"
            | "cdma"
            | "lte"
            | "4g"
            | "5g"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan() -> WirelessLan {
        WirelessLan {
            ssid: "Office".into(),
            status: "active".into(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        }
    }

    #[test]
    fn wireless_lan_rejects_invalid_scope_and_auth_without_echoing_psk() {
        let mut value = lan();
        assert!(value.validate().is_ok());
        value.scope_type = Some("dcim.site".into());
        assert!(value.validate().is_err());
        value.scope_id = Some(Uuid::now_v7());
        assert!(value.validate().is_ok());
        value.scope_type = Some("users.owner".into());
        assert!(value.validate().is_err());
        value.scope_type = None;
        value.scope_id = None;
        value.auth_psk = "x".repeat(65);
        let error = value.validate().unwrap_err().to_string();
        assert!(error.contains("auth_psk"));
        assert!(!error.contains(&value.auth_psk));
    }

    #[test]
    fn wireless_link_rejects_same_endpoints_invalid_distance_and_wired_type() {
        let mut value = WirelessLink {
            interface_a_id: Uuid::now_v7(),
            interface_b_id: Uuid::now_v7(),
            status: "connected".into(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        assert!(value.validate().is_ok());
        value.interface_b_id = value.interface_a_id;
        assert!(value.validate().is_err());
        value.interface_b_id = Uuid::now_v7();
        value.distance = Some(f64::NAN);
        assert!(value.validate().is_err());
        value.distance = Some(12.5);
        assert!(value.validate().is_err());
        value.distance_unit = Some("m".into());
        assert!(value.validate().is_ok());
        value.distance = Some(1.234);
        assert!(value.validate().is_err());
        assert!(is_wireless_interface("ieee802.11ax"));
        assert!(!is_wireless_interface("1000base-t"));
    }

    #[test]
    fn wireless_group_rejects_self_parent_and_invalid_slug() {
        let mut value = WirelessLanGroup {
            id: Uuid::now_v7(),
            name: "Office".into(),
            slug: "office".into(),
            custom_fields: serde_json::json!({}),
            ..Default::default()
        };
        assert!(value.validate().is_ok());
        value.parent_id = Some(value.id);
        assert!(value.validate().is_err());
        value.parent_id = None;
        value.slug = "not a slug".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn sparse_writes_preserve_omissions_clear_nullable_fields_and_reject_unknowns() {
        let group = Uuid::now_v7();
        let mut value = lan();
        value.group_id = Some(group);
        value.comments = "keep".into();
        let patch: WirelessLanWrite =
            serde_json::from_value(serde_json::json!({"description":"changed"})).unwrap();
        patch.apply(&mut value).unwrap();
        assert_eq!(value.group_id, Some(group));
        assert_eq!(value.comments, "keep");
        let patch: WirelessLanWrite =
            serde_json::from_value(serde_json::json!({"group_id":null})).unwrap();
        patch.apply(&mut value).unwrap();
        assert_eq!(value.group_id, None);
        for payload in [
            serde_json::json!({"tags":null}),
            serde_json::json!({"ssid":null}),
            serde_json::json!({"id":Uuid::now_v7()}),
            serde_json::json!({"scope_type_id":Uuid::now_v7()}),
        ] {
            assert!(serde_json::from_value::<WirelessLanWrite>(payload).is_err());
        }
    }
}
