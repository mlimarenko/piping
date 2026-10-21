use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::custom_fields::domain::{
    CustomFieldDefinition, CustomFieldType, validate_value,
};
use crate::contexts::custom_fields::infrastructure::{
    CustomFieldChoiceSetRepository, CustomFieldDefinitionRepository,
};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE, SecretFieldIntent,
    SecretFieldIntentService, SecretStore,
};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;

/// Owner scope for inventory-custom-field secrets: the entity owns its own
/// secret values directly (no scope-hierarchy merge), so the owner scope is the
/// entity itself.
const OWNER_SCOPE_TYPE: &str = INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE;
const SECRET_MASK: &str = "****";

/// One resolved custom field for an entity, ready for a response map.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCustomField {
    pub name: String,
    pub group_name: String,
    pub field_type: CustomFieldType,
    pub is_secret: bool,
    /// The value to surface: the typed non-secret value, the secret mask
    /// (`****`), the revealed secret cleartext, the default, or `null`.
    pub value: Value,
}

/// The materialized custom fields of one entity, ordered by definition weight.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomFieldsView {
    pub fields: Vec<ResolvedCustomField>,
}

impl CustomFieldsView {
    /// Project to the flat `name -> value` map embedded on inventory responses.
    pub fn to_value_map(&self) -> Value {
        let mut map = Map::new();
        for field in &self.fields {
            map.insert(field.name.clone(), field.value.clone());
        }
        Value::Object(map)
    }
}

/// The outcome of validating + splitting an incoming custom-fields write.
pub struct WriteOutcome {
    /// The sanitized `custom_field_data` JSONB the inventory repository persists
    /// (non-secret typed values only; secret keys stripped).
    pub custom_field_data: Value,
    /// Secret-field intents to apply against the secrets context after the
    /// entity row is persisted.
    pub secret_intents: Vec<SecretWriteIntent>,
}

/// A single secret custom-field value to push into the secrets context.
pub struct SecretWriteIntent {
    pub field_key: String,
    pub intent: SecretFieldIntent,
    pub value: Option<String>,
}

/// Object-safe surface over the custom-field write path, for callers that
/// cannot name the concrete [`CustomFieldValueService`]'s four repository
/// generics (e.g. the bulk import transaction service, which takes this as a
/// `&dyn CustomFieldWriter` call parameter). The two methods mirror the
/// inherent ones on the concrete service.
#[async_trait]
pub trait CustomFieldWriter: Send + Sync {
    /// Validate + split an incoming custom-fields write, keyed by the
    /// `app_label.model` content-type token. Returns the sanitized non-secret
    /// JSONB to persist and the secret intents to apply once the entity exists.
    async fn write_for_entity_token(
        &self,
        content_type_token: &str,
        incoming: &Map<String, Value>,
    ) -> Result<WriteOutcome, AppError>;

    /// Apply secret intents against the secrets context, keyed to a now-persisted
    /// entity id. MUST be called only after the entity row exists.
    async fn apply_secret_intents(
        &self,
        entity_id: Uuid,
        intents: Vec<SecretWriteIntent>,
    ) -> Result<(), AppError>;
}

/// The JOIN layer between custom-field definitions, the entity's plaintext
/// `custom_field_data` JSONB, and the secrets context.
pub struct CustomFieldValueService<D, C, R, S>
where
    D: CustomFieldDefinitionRepository,
    C: CustomFieldChoiceSetRepository,
    R: SecretRepository,
    S: SecretStore,
{
    definitions: Arc<D>,
    choice_sets: Arc<C>,
    secrets: Arc<R>,
    store: Arc<S>,
    secret_intents: Arc<SecretFieldIntentService<R>>,
}

impl<D, C, R, S> CustomFieldValueService<D, C, R, S>
where
    D: CustomFieldDefinitionRepository,
    C: CustomFieldChoiceSetRepository,
    R: SecretRepository,
    S: SecretStore,
{
    pub fn new(
        definitions: Arc<D>,
        choice_sets: Arc<C>,
        secrets: Arc<R>,
        store: Arc<S>,
        secret_intents: Arc<SecretFieldIntentService<R>>,
    ) -> Self {
        Self {
            definitions,
            choice_sets,
            secrets,
            store,
            secret_intents,
        }
    }

    /// Resolve a seeded content-type id from its `app_label.model` token (e.g.
    /// `"dcim.site"`). An unknown/unseeded token is a client error.
    async fn resolve_content_type(&self, content_type_token: &str) -> Result<Uuid, AppError> {
        let (app_label, model) = content_type_token.split_once('.').ok_or_else(|| {
            AppError::Validation(format!(
                "invalid content type token '{content_type_token}' (expected 'app_label.model')"
            ))
        })?;
        self.definitions
            .resolve_content_type_id(app_label, model)
            .await?
            .ok_or_else(|| {
                AppError::Validation(format!("unknown content type '{content_type_token}'"))
            })
    }

    /// Token-keyed [`read_for_entity`](Self::read_for_entity): resolves the
    /// `app_label.model` content-type token to its seeded id, then delegates.
    pub async fn read_for_entity_token(
        &self,
        content_type_token: &str,
        entity_id: Uuid,
        raw_custom_field_data: &Value,
        can_read_secrets: bool,
    ) -> Result<CustomFieldsView, AppError> {
        let content_type_id = self.resolve_content_type(content_type_token).await?;
        self.read_for_entity(
            content_type_id,
            entity_id,
            raw_custom_field_data,
            can_read_secrets,
        )
        .await
    }

    /// Token-keyed [`write_for_entity`](Self::write_for_entity).
    pub async fn write_for_entity_token(
        &self,
        content_type_token: &str,
        incoming: &Map<String, Value>,
    ) -> Result<WriteOutcome, AppError> {
        let content_type_id = self.resolve_content_type(content_type_token).await?;
        self.write_for_entity(content_type_id, incoming).await
    }

    /// Token-keyed reveal of every secret custom field of one entity, after the
    /// caller's per-entity access check has passed. Masked (non-secret) values
    /// are kept; secret values are replaced with their resolved cleartext (or
    /// `null` when unset). Used by the reveal endpoint.
    pub async fn reveal_for_entity_token(
        &self,
        content_type_token: &str,
        entity_id: Uuid,
        raw_custom_field_data: &Value,
    ) -> Result<CustomFieldsView, AppError> {
        let content_type_id = self.resolve_content_type(content_type_token).await?;
        self.read_for_entity(content_type_id, entity_id, raw_custom_field_data, true)
            .await
    }

    /// Materialize the custom fields of one entity for a response.
    ///
    /// `raw_custom_field_data` is the entity's stored plaintext JSONB. Secret
    /// fields never appear there; their presence is read from the secrets
    /// binding. When `can_read_secrets` is false a present secret is masked to
    /// `****`; when true it is revealed via the secrets store. Missing values
    /// fall back to the definition default (`null` when none).
    /// Materialize masked custom fields for a WHOLE PAGE of entities.
    ///
    /// The per-entity [`read_for_entity_token`](Self::read_for_entity_token)
    /// inside a list loop was the dominant N+1 in inventory lists: on the
    /// live schema every device row cost 31 queries (content-type lookup +
    /// definitions + one binding probe per secret field), making the masking
    /// pass ~4x more expensive than the page query itself. This resolves the
    /// content type once, loads definitions once, and fetches every secret
    /// binding for the page in one `ANY($2)` query — 3 queries per page,
    /// independent of page size.
    ///
    /// Masked-only by design (`can_read_secrets = false` semantics): list
    /// responses never carry cleartext, so no reveal path exists here.
    pub async fn read_for_entities_token_masked(
        &self,
        content_type_token: &str,
        entities: &[(Uuid, Value)],
    ) -> Result<Vec<CustomFieldsView>, AppError> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let content_type_id = self.resolve_content_type(content_type_token).await?;
        let definitions = self
            .definitions
            .list_for_content_type(content_type_id)
            .await?;

        let has_secret_definitions = definitions.iter().any(|def| def.is_secret);
        let mut bound: std::collections::HashSet<(Uuid, &str)> = std::collections::HashSet::new();
        let bindings;
        if has_secret_definitions {
            let ids: Vec<Uuid> = entities.iter().map(|(id, _)| *id).collect();
            bindings = self
                .secrets
                .list_usage_bindings_for_consumers(INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE, &ids)
                .await?;
            for binding in &bindings {
                bound.insert((binding.consumer_id, binding.field_key.as_str()));
            }
        }

        let mut views = Vec::with_capacity(entities.len());
        for (entity_id, raw) in entities {
            let stored = raw.as_object();
            let mut fields = Vec::with_capacity(definitions.len());
            for def in &definitions {
                let value = resolve_masked_field_value(
                    &def.name,
                    def.is_secret,
                    stored,
                    def.default_value.as_ref(),
                    bound.contains(&(*entity_id, def.name.as_str())),
                );
                fields.push(ResolvedCustomField {
                    name: def.name.clone(),
                    group_name: def.group_name.clone(),
                    field_type: def.field_type,
                    is_secret: def.is_secret,
                    value,
                });
            }
            views.push(CustomFieldsView { fields });
        }
        Ok(views)
    }

    pub async fn read_for_entity(
        &self,
        content_type_id: Uuid,
        entity_id: Uuid,
        raw_custom_field_data: &Value,
        can_read_secrets: bool,
    ) -> Result<CustomFieldsView, AppError> {
        let definitions = self
            .definitions
            .list_for_content_type(content_type_id)
            .await?;
        let stored = raw_custom_field_data.as_object();

        let mut fields = Vec::with_capacity(definitions.len());
        for def in definitions {
            let value = if def.is_secret {
                self.resolve_secret_value(entity_id, &def, can_read_secrets)
                    .await?
            } else {
                stored
                    .and_then(|map| map.get(&def.name))
                    .cloned()
                    .or_else(|| def.default_value.clone())
                    .unwrap_or(Value::Null)
            };
            fields.push(ResolvedCustomField {
                name: def.name,
                group_name: def.group_name,
                field_type: def.field_type,
                is_secret: def.is_secret,
                value,
            });
        }

        Ok(CustomFieldsView { fields })
    }

    async fn resolve_secret_value(
        &self,
        entity_id: Uuid,
        def: &CustomFieldDefinition,
        can_read_secrets: bool,
    ) -> Result<Value, AppError> {
        let binding = self
            .secrets
            .get_usage_binding(INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE, entity_id, &def.name)
            .await?;
        if binding.is_none() {
            return Ok(Value::Null);
        }
        if !can_read_secrets {
            return Ok(Value::String(SECRET_MASK.to_string()));
        }
        match self.reveal_secret(entity_id, &def.name).await? {
            Some(cleartext) => Ok(Value::String(cleartext)),
            None => Ok(Value::String(SECRET_MASK.to_string())),
        }
    }

    /// Resolve the cleartext of one secret custom field after the caller's
    /// per-entity access check has passed. Returns `None` when no secret is set.
    ///
    /// The `secret_usage_binding` is the authority that the field is a secret;
    /// the cleartext is read from the secrets store (the same path the intent
    /// service writes through).
    pub async fn reveal_secret(
        &self,
        entity_id: Uuid,
        field_key: &str,
    ) -> Result<Option<String>, AppError> {
        let binding = self
            .secrets
            .get_usage_binding(INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE, entity_id, field_key)
            .await?;
        let Some(binding) = binding else {
            return Ok(None);
        };
        match self.store.get_secret(binding.secret_id).await {
            Ok(value) if !value.is_empty() => Ok(Some(value)),
            Ok(_) => Ok(None),
            Err(AppError::MissingSecret(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Validate and split an incoming custom-fields write into the sanitized
    /// plaintext JSONB and the secret intents to apply.
    ///
    /// Unknown keys and keys not scoped to this content type are rejected.
    /// Secret values are routed to the secrets context; their keys are stripped
    /// from the JSONB. Non-secret values are type-validated and kept.
    pub async fn write_for_entity(
        &self,
        content_type_id: Uuid,
        incoming: &Map<String, Value>,
    ) -> Result<WriteOutcome, AppError> {
        let definitions = self
            .definitions
            .list_for_content_type(content_type_id)
            .await?;
        let by_name: BTreeMap<&str, &CustomFieldDefinition> = definitions
            .iter()
            .map(|def| (def.name.as_str(), def))
            .collect();

        for key in incoming.keys() {
            if !by_name.contains_key(key.as_str()) {
                return Err(AppError::Validation(format!(
                    "unknown or out-of-scope custom field '{key}'"
                )));
            }
        }

        let mut sanitized = Map::new();
        let mut secret_intents = Vec::new();

        for (name, def) in &by_name {
            let Some(value) = incoming.get(*name) else {
                continue;
            };

            // A read masks a set secret to `****`; a read-modify-write client
            // that PUTs the object back unchanged would otherwise round-trip the
            // mask as a real value and overwrite the cleartext with `****`. Treat
            // an incoming mask on a secret field as "keep existing": emit no
            // intent. A blank value still maps to Clear (the delete path).
            if def.is_secret && value.as_str() == Some(SECRET_MASK) {
                continue;
            }

            let choice_values = self.choice_values_for(def).await?;
            validate_value(def, value, &choice_values).map_err(AppError::from)?;

            if def.is_secret {
                secret_intents.push(secret_intent_for(name, value));
            } else {
                sanitized.insert((*name).to_string(), value.clone());
            }
        }

        Ok(WriteOutcome {
            custom_field_data: Value::Object(sanitized),
            secret_intents,
        })
    }

    /// Reject a create whose payload omits a `required` field. `write_for_entity`
    /// only validates values that are present; on create (and only create) every
    /// required field applicable to the content type must carry a non-empty
    /// value. Update is an explicit partial merge and does not call this.
    pub async fn check_required_present(
        &self,
        content_type_token: &str,
        incoming: &Map<String, Value>,
    ) -> Result<(), AppError> {
        let content_type_id = self.resolve_content_type(content_type_token).await?;
        let definitions = self
            .definitions
            .list_for_content_type(content_type_id)
            .await?;
        for def in &definitions {
            if !def.required {
                continue;
            }
            let present = match incoming.get(&def.name) {
                None | Some(Value::Null) => false,
                Some(Value::String(text)) => !text.trim().is_empty(),
                Some(Value::Array(items)) => !items.is_empty(),
                Some(_) => true,
            };
            if !present {
                return Err(AppError::Validation(format!(
                    "required custom field '{}' is missing",
                    def.name
                )));
            }
        }
        Ok(())
    }

    /// Apply the secret intents produced by [`write_for_entity`] for one entity.
    pub async fn apply_secret_intents(
        &self,
        entity_id: Uuid,
        intents: Vec<SecretWriteIntent>,
    ) -> Result<(), AppError> {
        for intent in intents {
            self.secret_intents
                .apply_intent(ApplySecretFieldIntentInput {
                    consumer_type: INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE.to_string(),
                    consumer_id: entity_id,
                    owner_scope_type: OWNER_SCOPE_TYPE.to_string(),
                    owner_scope_id: entity_id,
                    field_key: intent.field_key,
                    intent: intent.intent,
                    value: intent.value,
                    secret_id: None,
                })
                .await?;
        }
        Ok(())
    }

    async fn choice_values_for(
        &self,
        def: &CustomFieldDefinition,
    ) -> Result<Vec<String>, AppError> {
        if !def.field_type.is_choice() {
            return Ok(Vec::new());
        }
        let Some(choice_set_id) = def.choice_set_id else {
            return Ok(Vec::new());
        };
        let Some(choice_set) = self.choice_sets.get(choice_set_id).await? else {
            return Ok(Vec::new());
        };
        Ok(choice_set
            .extra_choices
            .into_iter()
            .map(|choice| choice.value)
            .collect())
    }
}

#[async_trait]
impl<D, C, R, S> CustomFieldWriter for CustomFieldValueService<D, C, R, S>
where
    D: CustomFieldDefinitionRepository,
    C: CustomFieldChoiceSetRepository,
    R: SecretRepository,
    S: SecretStore,
{
    async fn write_for_entity_token(
        &self,
        content_type_token: &str,
        incoming: &Map<String, Value>,
    ) -> Result<WriteOutcome, AppError> {
        CustomFieldValueService::write_for_entity_token(self, content_type_token, incoming).await
    }

    async fn apply_secret_intents(
        &self,
        entity_id: Uuid,
        intents: Vec<SecretWriteIntent>,
    ) -> Result<(), AppError> {
        CustomFieldValueService::apply_secret_intents(self, entity_id, intents).await
    }
}

fn resolve_masked_field_value(
    field_name: &str,
    is_secret: bool,
    stored: Option<&Map<String, Value>>,
    default_value: Option<&Value>,
    has_secret_binding: bool,
) -> Value {
    let stored_value = stored.and_then(|map| map.get(field_name)).cloned();
    let candidates = [
        (
            is_secret && has_secret_binding,
            Value::String(SECRET_MASK.to_string()),
        ),
        (
            !is_secret && stored_value.is_some(),
            stored_value.unwrap_or(Value::Null),
        ),
        (
            !is_secret && default_value.is_some(),
            default_value.cloned().unwrap_or(Value::Null),
        ),
        (true, Value::Null),
    ];

    // First-match order is the invariant: bindings mask secrets; secret fields
    // never consult JSON/defaults; stored non-secret values (including null)
    // outrank defaults; the final row supplies the null fallback.
    candidates
        .into_iter()
        .find(|(matches, _)| *matches)
        .map(|(_, value)| value)
        .unwrap_or(Value::Null)
}

fn secret_intent_for(name: &str, value: &Value) -> SecretWriteIntent {
    let cleartext = value.as_str().map(str::to_string);
    let is_blank = cleartext
        .as_ref()
        .map(|text| text.is_empty())
        .unwrap_or(true);
    if is_blank {
        SecretWriteIntent {
            field_key: name.to_string(),
            intent: SecretFieldIntent::Clear,
            value: None,
        }
    } else {
        SecretWriteIntent {
            field_key: name.to_string(),
            intent: SecretFieldIntent::Set,
            value: cleartext,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SECRET_MASK, resolve_masked_field_value};

    #[test]
    fn masked_secret_value_depends_only_on_binding_presence() {
        let raw = json!({"credential": "must-not-leak"});
        let default = json!("must-not-default");
        let cases = [(false, serde_json::Value::Null), (true, json!(SECRET_MASK))];

        for (has_secret_binding, expected) in cases {
            assert_eq!(
                resolve_masked_field_value(
                    "credential",
                    true,
                    raw.as_object(),
                    Some(&default),
                    has_secret_binding,
                ),
                expected
            );
        }
    }

    #[test]
    fn masked_non_secret_value_prefers_stored_then_default_then_null() {
        let raw = json!({"stored": "stored-value", "explicit-null": null});
        let default = json!("default-value");
        let cases = [
            ("stored", Some(&default), json!("stored-value")),
            ("explicit-null", Some(&default), serde_json::Value::Null),
            ("missing", Some(&default), json!("default-value")),
            ("missing", None, serde_json::Value::Null),
        ];

        for (field_name, default_value, expected) in cases {
            assert_eq!(
                resolve_masked_field_value(
                    field_name,
                    false,
                    raw.as_object(),
                    default_value,
                    false,
                ),
                expected
            );
        }
    }
}
