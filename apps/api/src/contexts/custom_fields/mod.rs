//! Custom fields bounded context.
//!
//! A NetBox-shaped, user-extensible schema layer over inventory entities.
//! Definitions, their object-type scope, and the choice sets that back
//! select/multiselect fields live here (migration `0056_init_custom_fields.sql`).
//!
//! VALUES are not owned by a table in this context: non-secret values stay in
//! each entity's existing `custom_field_data` JSONB (keyed by field name), and
//! secret values are stored through the secrets context
//! (`secret_usage_binding`, consumer type `inventory_custom_field`).

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::{
    CustomFieldDefinitionService, CustomFieldValueService, CustomFieldWriter, CustomFieldsView,
    ResolvedCustomField, SecretWriteIntent, WriteOutcome, can_read_entity_secret,
};
pub use domain::{
    Choice, CustomFieldChoiceSet, CustomFieldDefinition, CustomFieldError, CustomFieldType,
    FilterLogic, NewCustomFieldChoiceSet, NewCustomFieldDefinition, UiEditable, UiVisible,
    UpdateCustomFieldChoiceSet, UpdateCustomFieldDefinition, is_valid_custom_field_key,
    validate_definition, validate_value,
};
pub use infrastructure::{
    CUSTOM_FIELD_CHOICE_SET_COLUMNS, CUSTOM_FIELD_DEFINITION_COLUMNS,
    CustomFieldChoiceSetListFilter, CustomFieldChoiceSetRepository,
    CustomFieldDefinitionListFilter, CustomFieldDefinitionRepository,
    SqlxCustomFieldChoiceSetRepository, SqlxCustomFieldDefinitionRepository,
};
