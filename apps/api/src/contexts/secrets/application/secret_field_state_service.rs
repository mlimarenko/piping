use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::secrets::application::{
    CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, SecretStore, normalize_secret_field_consumer_type,
};
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretFieldState {
    pub consumer_type: String,
    pub consumer_id: Uuid,
    pub field_key: String,
    pub owner_scope_type: String,
    pub owner_scope_id: Uuid,
    pub enabled: bool,
    pub secret_id: Option<Uuid>,
    pub has_value: bool,
}

/// A single resolved credential field of a `connconf_record`. A field is a
/// secret reference iff a `secret_usage_binding` (consumer
/// `connection_config:record`, `field_key` = the dotted credential path) exists
/// for it — the binding layer IS the authority, not any inline UUID heuristic.
#[derive(Debug, Clone)]
pub struct RecordCredentialField {
    /// Dotted credential path, e.g. `credentials.private_key`.
    pub field_key: String,
    /// Whether this field is backed by a secret reference (binding present).
    pub is_secret_ref: bool,
    /// The resolved plaintext secret value. Populated only when the field is a
    /// secret reference AND the caller may view secrets (`can_view_secrets`).
    pub value: Option<String>,
}

pub struct SecretFieldStateService<R, S>
where
    R: SecretRepository,
    S: SecretStore,
{
    repository: Arc<R>,
    store: Arc<S>,
}

impl<R, S> SecretFieldStateService<R, S>
where
    R: SecretRepository,
    S: SecretStore,
{
    pub fn new(repository: Arc<R>, store: Arc<S>) -> Self {
        Self { repository, store }
    }

    pub async fn list_for_consumer(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
    ) -> Result<Vec<SecretFieldState>, AppError> {
        let consumer_type = normalize_secret_field_consumer_type(consumer_type)?;
        let bindings = self
            .repository
            .list_usage_bindings_for_consumer(&consumer_type, consumer_id)
            .await?;

        let mut states = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let has_value = match self.store.get_secret(binding.secret_id).await {
                Ok(value) => !value.trim().is_empty(),
                Err(AppError::MissingSecret(_)) => false,
                Err(error) => return Err(error),
            };
            states.push(SecretFieldState {
                consumer_type: binding.consumer_type,
                consumer_id: binding.consumer_id,
                field_key: binding.field_key,
                owner_scope_type: binding.owner_scope_type,
                owner_scope_id: binding.owner_scope_id,
                enabled: true,
                secret_id: Some(binding.secret_id),
                has_value,
            });
        }

        Ok(states)
    }

    /// Reveal the resolved credential fields of a `connconf_record`.
    ///
    /// Each `secret_usage_binding` for consumer `connection_config:record` and
    /// this `record_id` marks one credential field as a secret reference — the
    /// SAME binding layer the connect path and effective-config resolver rely on
    /// (no inline UUID regex). When `can_view_secrets` is true, the bound
    /// secret's plaintext is resolved through the store; otherwise the value is
    /// withheld (`None`). Literal credential fields have no binding and are not
    /// returned here — the client already holds their values.
    ///
    /// SQL lives in the repository (`list_usage_bindings_for_consumer`); this
    /// method is the thin application orchestration the handler delegates to.
    pub async fn reveal_record_credentials(
        &self,
        record_id: Uuid,
        can_view_secrets: bool,
    ) -> Result<Vec<RecordCredentialField>, AppError> {
        let bindings = self
            .repository
            .list_usage_bindings_for_consumer(CONNECTION_CONFIG_RECORD_CONSUMER_TYPE, record_id)
            .await?;

        let mut fields = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let value = if can_view_secrets {
                match self.store.get_secret(binding.secret_id).await {
                    Ok(value) => Some(value),
                    Err(AppError::MissingSecret(_)) => None,
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            fields.push(RecordCredentialField {
                field_key: binding.field_key,
                is_secret_ref: true,
                value,
            });
        }

        Ok(fields)
    }
}
