//! Secret resolution, storage and field-intent application services with their re-exports.
pub mod consumer_type;
pub mod secret_field_intent_service;
pub mod secret_field_state_service;
pub mod secret_resolver;
pub mod secret_store;

pub use consumer_type::{
    ASSISTANT_INSTANCE_CONSUMER_TYPE, CONNECTION_CONFIG_RECORD_CONSUMER_TYPE,
    CONNECTION_TARGET_SECRET_CONSUMER_TYPE, INVENTORY_CUSTOM_FIELD_CONSUMER_TYPE,
    PLAYBOOK_PROJECT_SCM_CREDENTIAL_CONSUMER_TYPE, PLAYBOOK_PROJECT_SCM_CREDENTIAL_FIELD_KEY,
    PLAYBOOK_SERVICE_PRINCIPAL_CREDENTIAL_CONSUMER_TYPE,
    PLAYBOOK_VERSION_RUNTIME_ENV_CONSUMER_TYPE, PROXY_NODE_CREDENTIAL_CONSUMER_TYPE,
    is_user_connection_credential_set_consumer_type, normalize_secret_field_consumer_type,
};
pub use secret_field_intent_service::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
pub use secret_field_state_service::{
    RecordCredentialField, SecretFieldState, SecretFieldStateService,
};
pub use secret_resolver::{SecretVisibilityChecker, resolve_secret_visibility, resolve_secrets};
pub use secret_store::SecretStore;

use async_trait::async_trait;

use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;

#[async_trait]
pub trait SecretFieldIntentApplier: Send + Sync {
    async fn apply_intent(
        &self,
        input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError>;
}

#[async_trait]
impl<R> SecretFieldIntentApplier for SecretFieldIntentService<R>
where
    R: SecretRepository + Send + Sync,
{
    async fn apply_intent(
        &self,
        input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError> {
        SecretFieldIntentService::apply_intent(self, input).await
    }
}
