//! Custom-fields infrastructure adapters and their persistence re-exports.
pub mod persistence;

pub use persistence::{
    CUSTOM_FIELD_CHOICE_SET_COLUMNS, CUSTOM_FIELD_DEFINITION_COLUMNS,
    CustomFieldChoiceSetListFilter, CustomFieldChoiceSetRepository,
    CustomFieldDefinitionListFilter, CustomFieldDefinitionRepository,
    SqlxCustomFieldChoiceSetRepository, SqlxCustomFieldDefinitionRepository,
};
