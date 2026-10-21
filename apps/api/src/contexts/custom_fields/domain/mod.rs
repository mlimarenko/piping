//! Custom-fields domain: field-type taxonomy, definitions, choice sets, and the
//! pure value/config validation that mirrors NetBox's `CustomField` model.

pub mod choice_set;
pub mod definition;
pub mod error;
pub mod field_type;
pub mod validation;

pub use choice_set::{
    Choice, CustomFieldChoiceSet, NewCustomFieldChoiceSet, UpdateCustomFieldChoiceSet,
};
pub use definition::{
    CustomFieldDefinition, FilterLogic, NewCustomFieldDefinition, UiEditable, UiVisible,
    UpdateCustomFieldDefinition,
};
pub use error::CustomFieldError;
pub use field_type::CustomFieldType;
pub use validation::{is_valid_custom_field_key, validate_definition, validate_value};
