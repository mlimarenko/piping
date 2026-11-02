use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::connections::domain::user_connection_credentials::{
    USER_CONNECTION_CREDENTIAL_OWNER_SCOPE_TYPE, USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
    is_user_connection_credential_field_key,
};
use crate::contexts::secrets::application::consumer_type::{
    FHRP_AUTH_KEY_FIELD, INVENTORY_FHRP_GROUP_CONSUMER_TYPE,
};
use crate::contexts::secrets::application::normalize_secret_field_consumer_type;
use crate::contexts::secrets::application::secret_field_state_service::SecretFieldState;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretFieldIntent {
    Set,
    Rotate,
    Clear,
}

impl SecretFieldIntent {
    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "set" => Ok(Self::Set),
            "rotate" => Ok(Self::Rotate),
            "clear" => Ok(Self::Clear),
            _ => Err(AppError::Validation(
                "intent must be one of: set, rotate, clear".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplySecretFieldIntentInput {
    pub consumer_type: String,
    pub consumer_id: Uuid,
    pub owner_scope_type: String,
    pub owner_scope_id: Uuid,
    pub field_key: String,
    pub intent: SecretFieldIntent,
    pub value: Option<String>,
    pub secret_id: Option<Uuid>,
}

impl ApplySecretFieldIntentInput {
    /// Pure native-consumer shape checks, shared by preflight and persistence.
    pub(crate) fn validate_native_shape(&self) -> Result<(), AppError> {
        if self.consumer_type != INVENTORY_FHRP_GROUP_CONSUMER_TYPE {
            return Ok(());
        }
        if self.owner_scope_type != INVENTORY_FHRP_GROUP_CONSUMER_TYPE
            || self.owner_scope_id != self.consumer_id
        {
            return Err(AppError::Validation(
                "Invalid FHRP secret field owner scope".into(),
            ));
        }
        if self.secret_id.is_some() {
            return Err(AppError::Validation(
                "FHRP auth keys cannot attach an existing secret".into(),
            ));
        }
        if self.field_key != FHRP_AUTH_KEY_FIELD {
            return Err(AppError::Validation("Unsupported FHRP secret field".into()));
        }
        if self
            .value
            .as_ref()
            .is_some_and(|value| value.chars().count() > 255)
        {
            return Err(AppError::Validation(
                "FHRP auth key exceeds 255 characters".into(),
            ));
        }
        Ok(())
    }
}

pub struct SecretFieldIntentService<R: SecretRepository> {
    repository: Arc<R>,
}

impl<R: SecretRepository> SecretFieldIntentService<R> {
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn apply_intent(
        &self,
        input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError> {
        let mut states = self.apply_intents(vec![input]).await?;
        states
            .pop()
            .ok_or_else(|| AppError::Internal("secret intent batch returned no state".into()))
    }

    /// Validates every input before applying the batch in one transaction.
    pub async fn apply_intents(
        &self,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Vec<SecretFieldState>, AppError> {
        let inputs = Self::validate_inputs(inputs)?;
        self.repository.apply_field_intents(inputs).await
    }

    /// Applies a validated batch without committing the caller's transaction.
    /// The caller must roll back its transaction if this operation fails.
    pub async fn apply_intents_in_transaction(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Vec<SecretFieldState>, AppError> {
        let inputs = Self::validate_inputs(inputs)?;
        self.repository
            .apply_field_intents_in_transaction(transaction, inputs)
            .await
    }

    pub(crate) fn validate_inputs(
        inputs: Vec<ApplySecretFieldIntentInput>,
    ) -> Result<Vec<ApplySecretFieldIntentInput>, AppError> {
        inputs.into_iter().map(Self::validate_input).collect()
    }

    pub(crate) fn validate_input(
        mut input: ApplySecretFieldIntentInput,
    ) -> Result<ApplySecretFieldIntentInput, AppError> {
        input.consumer_type = normalize_secret_field_consumer_type(&input.consumer_type)?;
        input.owner_scope_type = normalize_required(&input.owner_scope_type, "owner_scope_type")?;
        input.field_key = normalize_required(&input.field_key, "field_key")?;
        input.validate_native_shape()?;
        if input.consumer_type == USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE {
            if input.owner_scope_type != USER_CONNECTION_CREDENTIAL_OWNER_SCOPE_TYPE {
                return Err(AppError::Validation(
                    "owner_scope_type must be user for user connection credentials".into(),
                ));
            }
            if !is_user_connection_credential_field_key(&input.field_key) {
                return Err(AppError::Validation(
                    "field_key is not allowed for user connection credentials".into(),
                ));
            }
        }
        if input.intent != SecretFieldIntent::Clear
            && input.secret_id.is_none()
            && input
                .value
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AppError::Validation(
                "value is required when creating a new secret field".into(),
            ));
        }
        Ok(input)
    }
}

fn normalize_required(raw: &str, field: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}
