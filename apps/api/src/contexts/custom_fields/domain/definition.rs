use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::field_type::CustomFieldType;

/// When a field is shown in the UI. Mirrors `custom_field.ui_visible` and the
/// `custom_field_ui_visible_valid` CHECK (`always` / `if-set` / `hidden`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiVisible {
    Always,
    IfSet,
    Hidden,
}

impl UiVisible {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::IfSet => "if-set",
            Self::Hidden => "hidden",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "always" => Some(Self::Always),
            "if-set" => Some(Self::IfSet),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// Whether a field is editable in the UI. Mirrors `custom_field.ui_editable`
/// and the `custom_field_ui_editable_valid` CHECK (`yes` / `no` / `hidden`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiEditable {
    Yes,
    No,
    Hidden,
}

impl UiEditable {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Hidden => "hidden",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "yes" => Some(Self::Yes),
            "no" => Some(Self::No),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// How a field participates in list filtering. Mirrors
/// `custom_field.filter_logic` and the `custom_field_filter_logic_valid` CHECK
/// (`disabled` / `loose` / `exact`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterLogic {
    Disabled,
    Loose,
    Exact,
}

impl FilterLogic {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Loose => "loose",
            Self::Exact => "exact",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "disabled" => Some(Self::Disabled),
            "loose" => Some(Self::Loose),
            "exact" => Some(Self::Exact),
            _ => None,
        }
    }
}

/// A custom-field definition. Maps the `custom_field` row plus its
/// `custom_field_object_type` M2M scope (`object_types`).
///
/// Numeric validators are stored in a `numeric(16,4)` column; the codebase reads
/// such columns as `f64`, so the validators are `Option<f64>` here (no extra
/// decimal dependency).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFieldDefinition {
    pub id: Uuid,
    pub name: String,
    pub label: String,
    pub field_type: CustomFieldType,
    pub description: String,
    pub group_name: String,
    pub required: bool,
    pub enforce_unique: bool,
    pub is_secret: bool,
    pub default_value: Option<Value>,
    pub weight: i32,
    pub validation_minimum: Option<f64>,
    pub validation_maximum: Option<f64>,
    pub validation_regex: String,
    pub validation_schema: Option<Value>,
    pub choice_set_id: Option<Uuid>,
    pub related_object_type_id: Option<Uuid>,
    pub related_object_filter: Option<Value>,
    pub ui_visible: UiVisible,
    pub ui_editable: UiEditable,
    pub is_cloneable: bool,
    pub search_weight: i32,
    pub filter_logic: FilterLogic,
    /// Inventory `content_type` ids this field is scoped to.
    pub object_types: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Fields a caller may set when creating a definition.
#[derive(Debug, Clone)]
pub struct NewCustomFieldDefinition {
    pub name: String,
    pub label: String,
    pub field_type: CustomFieldType,
    pub description: String,
    pub group_name: String,
    pub required: bool,
    pub enforce_unique: bool,
    pub is_secret: bool,
    pub default_value: Option<Value>,
    pub weight: i32,
    pub validation_minimum: Option<f64>,
    pub validation_maximum: Option<f64>,
    pub validation_regex: String,
    pub validation_schema: Option<Value>,
    pub choice_set_id: Option<Uuid>,
    pub related_object_type_id: Option<Uuid>,
    pub related_object_filter: Option<Value>,
    pub ui_visible: UiVisible,
    pub ui_editable: UiEditable,
    pub is_cloneable: bool,
    pub search_weight: i32,
    pub filter_logic: FilterLogic,
    pub object_types: Vec<Uuid>,
}

/// Fields a caller may patch on an existing definition. `None` leaves a column
/// untouched; `object_types` replaces the scope set when present.
#[derive(Debug, Clone, Default)]
pub struct UpdateCustomFieldDefinition {
    pub label: Option<String>,
    pub field_type: Option<CustomFieldType>,
    pub description: Option<String>,
    pub group_name: Option<String>,
    pub required: Option<bool>,
    pub enforce_unique: Option<bool>,
    pub is_secret: Option<bool>,
    pub default_value: Option<Option<Value>>,
    pub weight: Option<i32>,
    pub validation_minimum: Option<Option<f64>>,
    pub validation_maximum: Option<Option<f64>>,
    pub validation_regex: Option<String>,
    pub validation_schema: Option<Option<Value>>,
    pub choice_set_id: Option<Option<Uuid>>,
    pub related_object_type_id: Option<Option<Uuid>>,
    pub related_object_filter: Option<Option<Value>>,
    pub ui_visible: Option<UiVisible>,
    pub ui_editable: Option<UiEditable>,
    pub is_cloneable: Option<bool>,
    pub search_weight: Option<i32>,
    pub filter_logic: Option<FilterLogic>,
    pub object_types: Option<Vec<Uuid>>,
}
