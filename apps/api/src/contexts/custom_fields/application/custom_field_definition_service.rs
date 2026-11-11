use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, NewCustomFieldDefinition, UpdateCustomFieldDefinition,
    validate_definition, validate_value,
};
use crate::contexts::custom_fields::infrastructure::{
    CustomFieldChoiceSetRepository, CustomFieldDefinitionListFilter,
    CustomFieldDefinitionRepository,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::ContentTypeRow;
use crate::contexts::shared::list_query::SortSpec;
use crate::errors::AppError;

/// CRUD orchestration for custom-field definitions. Runs the pure
/// [`validate_definition`] config invariants before any persist, and resolves a
/// choice set when a select/multiselect default must be a member of it (the only
/// invariant the pure layer cannot check without hydrated choices).
pub struct CustomFieldDefinitionService<D, C>
where
    D: CustomFieldDefinitionRepository,
    C: CustomFieldChoiceSetRepository,
{
    definitions: Arc<D>,
    choice_sets: Arc<C>,
}

impl<D, C> CustomFieldDefinitionService<D, C>
where
    D: CustomFieldDefinitionRepository,
    C: CustomFieldChoiceSetRepository,
{
    pub fn new(definitions: Arc<D>, choice_sets: Arc<C>) -> Self {
        Self {
            definitions,
            choice_sets,
        }
    }

    pub async fn list_page(
        &self,
        filter: &CustomFieldDefinitionListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CustomFieldDefinition>, AppError> {
        self.definitions.list_page(filter, limit, offset).await
    }

    pub async fn count(&self, filter: &CustomFieldDefinitionListFilter) -> Result<u64, AppError> {
        self.definitions.count(filter).await
    }

    pub async fn list_for_content_type(
        &self,
        content_type_id: Uuid,
    ) -> Result<Vec<CustomFieldDefinition>, AppError> {
        self.definitions
            .list_for_content_type(content_type_id)
            .await
    }

    pub async fn resolve_content_type_id(
        &self,
        app_label: &str,
        model: &str,
    ) -> Result<Option<Uuid>, AppError> {
        self.definitions
            .resolve_content_type_id(app_label, model)
            .await
    }

    /// List every seeded inventory content type for the scope picker.
    pub async fn list_content_types(
        &self,
        sort: Option<SortSpec>,
        app_labels: &[String],
        models: &[String],
    ) -> Result<Vec<ContentTypeRow>, AppError> {
        self.definitions
            .list_content_types(sort, app_labels, models)
            .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<CustomFieldDefinition>, AppError> {
        self.definitions.get(id).await
    }

    pub async fn create(
        &self,
        input: NewCustomFieldDefinition,
    ) -> Result<CustomFieldDefinition, AppError> {
        let candidate = self.draft_from_new(&input);
        validate_definition(&candidate)?;
        self.validate_default_membership(&candidate).await?;
        self.definitions.create(&input).await
    }

    pub async fn update(
        &self,
        id: Uuid,
        input: UpdateCustomFieldDefinition,
    ) -> Result<Option<CustomFieldDefinition>, AppError> {
        let Some(existing) = self.definitions.get(id).await? else {
            return Ok(None);
        };
        // Changing `is_secret` on an existing field would either leave already
        // stored plaintext in the entity JSONB (false->true) or strand a secret
        // binding (true->false). A safe in-place migration would have to rewrite
        // every scoped entity's value storage; until that exists, reject the
        // toggle and require delete+recreate so plaintext is never left exposed.
        if let Some(requested_secret) = input.is_secret
            && requested_secret != existing.is_secret
        {
            return Err(AppError::Validation(
                "changing the secret flag on an existing custom field is not supported; \
                     delete and recreate the field to change its secrecy"
                    .to_string(),
            ));
        }
        let candidate = apply_update(existing, &input);
        validate_definition(&candidate)?;
        self.validate_default_membership(&candidate).await?;
        self.definitions.update(id, &input).await
    }

    pub async fn delete(&self, id: Uuid) -> Result<bool, AppError> {
        self.definitions.delete(id).await
    }

    /// Build the post-create definition shape for validation (id/timestamps are
    /// placeholders — they do not affect any invariant).
    fn draft_from_new(&self, input: &NewCustomFieldDefinition) -> CustomFieldDefinition {
        let now = chrono::Utc::now();
        CustomFieldDefinition {
            id: Uuid::nil(),
            name: input.name.clone(),
            label: input.label.clone(),
            field_type: input.field_type,
            description: input.description.clone(),
            group_name: input.group_name.clone(),
            required: input.required,
            enforce_unique: input.enforce_unique,
            is_secret: input.is_secret,
            default_value: input.default_value.clone(),
            weight: input.weight,
            validation_minimum: input.validation_minimum,
            validation_maximum: input.validation_maximum,
            validation_regex: input.validation_regex.clone(),
            validation_schema: input.validation_schema.clone(),
            choice_set_id: input.choice_set_id,
            related_object_type_id: input.related_object_type_id,
            related_object_filter: input.related_object_filter.clone(),
            ui_visible: input.ui_visible,
            ui_editable: input.ui_editable,
            is_cloneable: input.is_cloneable,
            search_weight: input.search_weight,
            filter_logic: input.filter_logic,
            object_types: input.object_types.clone(),
            created_at: now,
            updated_at: now,
        }
    }

    /// For select/multiselect fields with a default, verify the default is a
    /// member of the resolved choice set. (`validate_definition` only checks the
    /// default's shape; membership needs the hydrated choices.)
    async fn validate_default_membership(
        &self,
        def: &CustomFieldDefinition,
    ) -> Result<(), AppError> {
        if !def.field_type.is_choice() {
            return Ok(());
        }
        let Some(default) = &def.default_value else {
            return Ok(());
        };
        if default.is_null() {
            return Ok(());
        }
        let Some(choice_set_id) = def.choice_set_id else {
            return Ok(());
        };
        let choice_set = self.choice_sets.get(choice_set_id).await?.ok_or_else(|| {
            AppError::Validation(format!("choice set {choice_set_id} does not exist"))
        })?;
        let choice_values: Vec<String> = choice_set
            .extra_choices
            .iter()
            .map(|choice| choice.value.clone())
            .collect();
        validate_value(def, default, &choice_values).map_err(AppError::from)
    }
}

/// Overwrite `target` with a clone of `source` when the patch carries a value.
/// Every optional field of [`UpdateCustomFieldDefinition`] follows this pattern,
/// so the branch lives here once instead of in each arm of `apply_update`.
fn apply_present<T: Clone>(target: &mut T, source: Option<&T>) {
    if let Some(value) = source {
        *target = value.clone();
    }
}

/// Apply a patch to an existing definition, producing the candidate that
/// validation runs against. `field_type` may not change once data exists, but
/// validation here is shape-only; the repository is the persistence authority.
fn apply_update(
    mut def: CustomFieldDefinition,
    input: &UpdateCustomFieldDefinition,
) -> CustomFieldDefinition {
    apply_present(&mut def.label, input.label.as_ref());
    apply_present(&mut def.field_type, input.field_type.as_ref());
    apply_present(&mut def.description, input.description.as_ref());
    apply_present(&mut def.group_name, input.group_name.as_ref());
    apply_present(&mut def.required, input.required.as_ref());
    apply_present(&mut def.enforce_unique, input.enforce_unique.as_ref());
    apply_present(&mut def.is_secret, input.is_secret.as_ref());
    apply_present(&mut def.default_value, input.default_value.as_ref());
    apply_present(&mut def.weight, input.weight.as_ref());
    apply_present(
        &mut def.validation_minimum,
        input.validation_minimum.as_ref(),
    );
    apply_present(
        &mut def.validation_maximum,
        input.validation_maximum.as_ref(),
    );
    apply_present(&mut def.validation_regex, input.validation_regex.as_ref());
    apply_present(&mut def.validation_schema, input.validation_schema.as_ref());
    apply_present(&mut def.choice_set_id, input.choice_set_id.as_ref());
    apply_present(
        &mut def.related_object_type_id,
        input.related_object_type_id.as_ref(),
    );
    apply_present(
        &mut def.related_object_filter,
        input.related_object_filter.as_ref(),
    );
    apply_present(&mut def.ui_visible, input.ui_visible.as_ref());
    apply_present(&mut def.ui_editable, input.ui_editable.as_ref());
    apply_present(&mut def.is_cloneable, input.is_cloneable.as_ref());
    apply_present(&mut def.search_weight, input.search_weight.as_ref());
    apply_present(&mut def.filter_logic, input.filter_logic.as_ref());
    apply_present(&mut def.object_types, input.object_types.as_ref());
    def
}
