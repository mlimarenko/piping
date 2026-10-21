use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::connections::domain::user_connection_credentials::{
    USER_CONNECTION_CREDENTIAL_OWNER_SCOPE_TYPE, USER_CONNECTION_CREDENTIAL_SET_CONSUMER_TYPE,
    is_user_connection_credential_field_key,
};
use crate::contexts::secrets::application::normalize_secret_field_consumer_type;
use crate::contexts::secrets::application::secret_field_state_service::SecretFieldState;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
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

pub struct SecretFieldIntentService<R: SecretRepository> {
    repository: Arc<R>,
}

impl<R: SecretRepository> SecretFieldIntentService<R> {
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn apply_intent(
        &self,
        mut input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError> {
        input.consumer_type = normalize_secret_field_consumer_type(&input.consumer_type)?;
        input.owner_scope_type = normalize_required(&input.owner_scope_type, "owner_scope_type")?;
        input.field_key = normalize_required(&input.field_key, "field_key")?;
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
        self.repository.apply_field_intent(input).await
    }
}

fn normalize_required(raw: &str, field: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}
