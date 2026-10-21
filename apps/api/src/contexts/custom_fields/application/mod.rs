//! Application layer for the custom-fields context.
//!
//! - [`CustomFieldDefinitionService`] — CRUD orchestration that runs the pure
//!   config-invariant validation before any persist.
//! - [`CustomFieldValueService`] — the JOIN layer between definitions, an
//!   entity's plaintext `custom_field_data` JSONB, and secret values stored
//!   through the secrets context.
//! - [`can_read_entity_secret`] — per-entity, attribute-based secret authz that
//!   reuses the existing access-control evaluator (no new `Action`).

pub mod custom_field_definition_service;
pub mod custom_field_value_service;
pub mod entity_secret_authz;

pub use custom_field_definition_service::CustomFieldDefinitionService;
pub use custom_field_value_service::{
    CustomFieldValueService, CustomFieldWriter, CustomFieldsView, ResolvedCustomField,
    SecretWriteIntent, WriteOutcome,
};
pub use entity_secret_authz::can_read_entity_secret;
