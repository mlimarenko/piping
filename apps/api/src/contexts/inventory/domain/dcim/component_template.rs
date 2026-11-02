//! Component template records and write contracts owned by DCIM.
//!
//! Nine NetBox 4.7.1 component-template models map onto the local
//! `dcim_*template` tables. `ConsolePortTemplate`, `ConsoleServerPortTemplate`,
//! `PowerPortTemplate`, `PowerOutletTemplate`, `InterfaceTemplate`,
//! `FrontPortTemplate` and `RearPortTemplate` are modular: exactly one of
//! `device_type_id` or `module_type_id` is set. `DeviceBayTemplate` and
//! `InventoryItemTemplate` belong to a device type only.
//!
//! The inventory-item tree's nested-set columns and the interface `_name`
//! ordering column are persistence bookkeeping and are intentionally not part
//! of the public record. Fields that the local schema does not carry (interface
//! `parent`, `channels`, `channel_id`; template
//! custom fields, tags and comments) are rejected at the write boundary instead
//! of being silently dropped.
//!
//! Front/rear wiring is persisted in `dcim_porttemplatemapping`. The front port
//! template owns the mapping rows: `FrontPortTemplateChanges::rear_ports`
//! replaces them atomically. `RearPortTemplate` exposes the resulting
//! `front_ports` read-only, so two endpoints never write the same rows.

pub mod choices;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

macro_rules! modular_template {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
        pub struct $name {
            pub id: Uuid,
            pub name: String,
            pub label: String,
            pub description: String,
            pub device_type_id: Option<Uuid>,
            pub module_type_id: Option<Uuid>,
            $(pub $field: $ty,)*
        }
    };
}

modular_template!(ConsolePortTemplate { r#type: Option<String> });
modular_template!(ConsoleServerPortTemplate { r#type: Option<String> });
modular_template!(PowerPortTemplate {
    r#type: Option<String>,
    maximum_draw: Option<i32>,
    allocated_draw: Option<i32>,
});
modular_template!(PowerOutletTemplate {
    r#type: Option<String>,
    color: String,
    power_port_id: Option<Uuid>,
    feed_leg: Option<String>,
});
modular_template!(InterfaceTemplate {
    r#type: String,
    enabled: bool,
    mgmt_only: bool,
    bridge_id: Option<Uuid>,
    poe_mode: Option<String>,
    poe_type: Option<String>,
    rf_role: Option<String>,
});

/// One front-port-to-rear-port position mapping, as exposed on a front port
/// template.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FrontPortMapping {
    /// Position on the front port template.
    pub position: i16,
    /// Rear port template the position terminates on.
    pub rear_port: Uuid,
    /// Position on that rear port template.
    pub rear_port_position: i16,
}

/// One rear-port-to-front-port position mapping, as exposed on a rear port
/// template. Mappings are written through the front port template.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RearPortMapping {
    /// Position on the front port template.
    pub position: i16,
    /// Front port template the position is wired to.
    pub front_port: Uuid,
    /// Position on that front port template.
    pub front_port_position: i16,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FrontPortTemplate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub device_type_id: Option<Uuid>,
    pub module_type_id: Option<Uuid>,
    pub r#type: String,
    pub color: String,
    pub positions: i16,
    pub rear_ports: Vec<FrontPortMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RearPortTemplate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub device_type_id: Option<Uuid>,
    pub module_type_id: Option<Uuid>,
    pub r#type: String,
    pub color: String,
    pub positions: i16,
    pub front_ports: Vec<RearPortMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeviceBayTemplate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub device_type_id: Uuid,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct InventoryItemTemplate {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub description: String,
    pub device_type_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub manufacturer_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
    pub part_id: String,
    pub component_type_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// Reject an explicit `null` for a field the local column stores as NOT NULL.
///
/// Serde only calls this adapter when the key is present; `#[serde(default)]`
/// handles absence before it. Omission therefore means "keep the current
/// value" on PUT, while `null` is a client mistake rather than a silent
/// omission.
pub(crate) fn non_null_patch<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("field must not be null"))
}

/// Preserve omission versus explicit null on a nullable column.
fn nullable_patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

/// One supplied front-port mapping. Owned by the front port template write
/// contract; `rear_ports` replaces the complete mapping set.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FrontPortMappingChanges {
    pub position: i16,
    pub rear_port: Uuid,
    pub rear_port_position: i16,
}

macro_rules! template_changes {
    ($name:ident { $($fields:tt)* }) => {
        template_changes!(@fields $name [] $($fields)*);
    };
    (@fields $name:ident [$($field:ident: $ty:ty => $adapter:literal, $nullable:literal;)*]) => {
        /// Omitted fields keep their current value on PUT; an explicit `null`
        /// is accepted only where the column is nullable.
        #[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                #[serde(
                    default,
                    skip_serializing_if = "Option::is_none",
                    deserialize_with = $adapter
                )]
                #[schema(nullable = $nullable)]
                pub $field: $ty,
            )*
        }
    };
    (@fields $name:ident [$($fields:tt)*]
        $field:ident: $ty:ty => "non_null_patch", $($rest:tt)*) => {
        template_changes!(@fields $name [
            $($fields)* $field: $ty => "non_null_patch", false;
        ] $($rest)*);
    };
    (@fields $name:ident [$($fields:tt)*]
        $field:ident: $ty:ty => "nullable_patch", $($rest:tt)*) => {
        template_changes!(@fields $name [
            $($fields)* $field: $ty => "nullable_patch", true;
        ] $($rest)*);
    };
}

template_changes!(ConsolePortTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<Option<String>> => "nullable_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(ConsoleServerPortTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<Option<String>> => "nullable_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(PowerPortTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<Option<String>> => "nullable_patch",
    maximum_draw: Option<Option<i32>> => "nullable_patch",
    allocated_draw: Option<Option<i32>> => "nullable_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(PowerOutletTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<Option<String>> => "nullable_patch",
    color: Option<String> => "non_null_patch",
    power_port_id: Option<Option<Uuid>> => "nullable_patch",
    feed_leg: Option<Option<String>> => "nullable_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(InterfaceTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<String> => "non_null_patch",
    enabled: Option<bool> => "non_null_patch",
    mgmt_only: Option<bool> => "non_null_patch",
    bridge_id: Option<Option<Uuid>> => "nullable_patch",
    poe_mode: Option<Option<String>> => "nullable_patch",
    poe_type: Option<Option<String>> => "nullable_patch",
    rf_role: Option<Option<String>> => "nullable_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(FrontPortTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<String> => "non_null_patch",
    color: Option<String> => "non_null_patch",
    positions: Option<i16> => "non_null_patch",
    rear_ports: Option<Vec<FrontPortMappingChanges>> => "non_null_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(RearPortTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    r#type: Option<String> => "non_null_patch",
    color: Option<String> => "non_null_patch",
    positions: Option<i16> => "non_null_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    module_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(DeviceBayTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    enabled: Option<bool> => "non_null_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
});

template_changes!(InventoryItemTemplateChanges {
    name: Option<String> => "non_null_patch",
    label: Option<String> => "non_null_patch",
    description: Option<String> => "non_null_patch",
    device_type_id: Option<Option<Uuid>> => "nullable_patch",
    parent_id: Option<Option<Uuid>> => "nullable_patch",
    manufacturer_id: Option<Option<Uuid>> => "nullable_patch",
    role_id: Option<Option<Uuid>> => "nullable_patch",
    part_id: Option<String> => "non_null_patch",
    component_type_id: Option<Option<Uuid>> => "nullable_patch",
    component_id: Option<Option<Uuid>> => "nullable_patch",
});

/// Maximum length of `name` and `label`, matching the local `varchar(64)`.
pub const TEMPLATE_NAME_MAX: usize = 64;
/// Maximum length of `description`, matching the local `varchar(200)`.
pub const TEMPLATE_DESCRIPTION_MAX: usize = 200;
/// NetBox `PORT_POSITION_MIN`.
pub const PORT_POSITION_MIN: i16 = 1;
/// NetBox `PORT_POSITION_MAX`.
pub const PORT_POSITION_MAX: i16 = 1024;

/// Validate an identity/short text column.
pub fn validate_text(field: &str, value: &str, max: usize, required: bool) -> Result<(), AppError> {
    if required && value.trim().is_empty() {
        return Err(AppError::Validation(format!("{field} must not be empty")));
    }
    if value.chars().count() > max {
        return Err(AppError::Validation(format!(
            "Invalid {field}: maximum {max} characters"
        )));
    }
    Ok(())
}

/// Validate a nullable choice column. NetBox marks several template choices
/// `allow_blank`, so `Some("")` is accepted exactly where `allow_blank` holds.
pub fn validate_choice(
    field: &str,
    value: Option<&str>,
    choices: &[&str],
    allow_blank: bool,
) -> Result<(), AppError> {
    match value {
        None => Ok(()),
        Some("") if allow_blank => Ok(()),
        Some(value) if choices.contains(&value) => Ok(()),
        Some(_) => Err(AppError::Validation(format!("Invalid {field}"))),
    }
}

/// Validate a required choice column (no blank, no null).
pub fn validate_required_choice(
    field: &str,
    value: &str,
    choices: &[&str],
) -> Result<(), AppError> {
    if choices.contains(&value) {
        Ok(())
    } else {
        Err(AppError::Validation(format!("Invalid {field}")))
    }
}

/// Validate a NetBox `ColorField`: blank, or six lowercase hexadecimal digits.
pub fn validate_color(field: &str, value: &str) -> Result<(), AppError> {
    let valid = value.is_empty()
        || (value.len() == 6
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    if valid {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "Invalid {field}: expected six lowercase hexadecimal digits"
        )))
    }
}

/// Validate a NetBox port position.
pub fn validate_positions(field: &str, value: i16) -> Result<(), AppError> {
    if (PORT_POSITION_MIN..=PORT_POSITION_MAX).contains(&value) {
        Ok(())
    } else {
        Err(AppError::Validation(format!(
            "Invalid {field}: expected {PORT_POSITION_MIN}..={PORT_POSITION_MAX}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_validation_rejects_blank_required_and_oversized_values() {
        assert!(validate_text("name", "  ", TEMPLATE_NAME_MAX, true).is_err());
        assert!(validate_text("name", "x", TEMPLATE_NAME_MAX, true).is_ok());
        assert!(validate_text("name", &"x".repeat(65), TEMPLATE_NAME_MAX, true).is_err());
        assert!(validate_text("label", "", TEMPLATE_NAME_MAX, false).is_ok());
    }

    #[test]
    fn choice_validation_matches_upstream_blank_policy() {
        assert!(validate_choice("type", Some(""), choices::CONSOLE_PORT_TYPES, true).is_ok());
        assert!(validate_choice("type", Some("rj-45"), choices::CONSOLE_PORT_TYPES, true).is_ok());
        assert!(validate_choice("type", Some("bogus"), choices::CONSOLE_PORT_TYPES, true).is_err());
        assert!(
            validate_required_choice("type", "", choices::INTERFACE_TYPES).is_err(),
            "required choice fields reject a blank value"
        );
    }

    #[test]
    fn color_validation_accepts_blank_or_lowercase_hex() {
        assert!(validate_color("color", "").is_ok());
        assert!(validate_color("color", "a1b2c3").is_ok());
        assert!(validate_color("color", "A1B2C3").is_err());
        assert!(validate_color("color", "a1b2c").is_err());
    }

    #[test]
    fn positions_validation_matches_netbox_bounds() {
        assert!(validate_positions("positions", 1).is_ok());
        assert!(validate_positions("positions", 1024).is_ok());
        assert!(validate_positions("positions", 0).is_err());
        assert!(validate_positions("positions", 1025).is_err());
    }
}
