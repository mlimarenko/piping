//! Application layer for the custom-fields context.
//!
//! - [`CustomFieldDefinitionService`] — CRUD orchestration that runs the pure
//!   config-invariant validation before any persist.
//! - [`CustomFieldValueService`] — the JOIN layer between definitions, an
//!   entity's plaintext `custom_field_data` JSONB, and secret values stored
//!   through the secrets context.

pub mod custom_field_definition_service;
pub mod custom_field_value_service;

pub use custom_field_definition_service::CustomFieldDefinitionService;
pub use custom_field_value_service::{
    CustomFieldValueService, CustomFieldWriter, CustomFieldsView, ResolvedCustomField,
    SecretWriteIntent, WriteOutcome,
};
