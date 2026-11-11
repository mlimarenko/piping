use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single selectable option backing a `select` / `multiselect` field. Mirrors
/// one `{"value": ..., "label": ...}` entry of `custom_field_choice_set.extra_choices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

/// A named set of choices backing `select` / `multiselect` custom fields. Maps
/// the `custom_field_choice_set` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomFieldChoiceSet {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// Ordered list of explicit choices (`extra_choices` JSON array).
    pub extra_choices: Vec<Choice>,
    pub order_alphabetically: bool,
    /// Optional map of choice value -> color token (`choice_colors`).
    pub choice_colors: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CustomFieldChoiceSet {
    /// True when `value` is one of the set's explicit choice values.
    pub fn contains_value(&self, value: &str) -> bool {
        self.extra_choices
            .iter()
            .any(|choice| choice.value == value)
    }
}

/// Fields a caller may set when creating a choice set.
#[derive(Debug, Clone)]
pub struct NewCustomFieldChoiceSet {
    pub name: String,
    pub description: String,
    pub extra_choices: Vec<Choice>,
    pub order_alphabetically: bool,
    pub choice_colors: BTreeMap<String, String>,
}

/// Fields a caller may patch on an existing choice set. `None` leaves a column
/// untouched.
#[derive(Debug, Clone, Default)]
pub struct UpdateCustomFieldChoiceSet {
    pub name: Option<String>,
    pub description: Option<String>,
    pub extra_choices: Option<Vec<Choice>>,
    pub order_alphabetically: Option<bool>,
    pub choice_colors: Option<BTreeMap<String, String>>,
}
