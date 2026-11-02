use std::sync::Arc;

use crate::config::AuthConfig;
use crate::contexts::access_control::domain::{
    OIDC_CLIENT_SECRET_CONSUMER_TYPE, OIDC_CLIENT_SECRET_FIELD_KEY, OidcClientSecretState,
    OidcProvisioningMode, OidcSettings, oidc_client_secret_consumer_id,
};
use crate::contexts::access_control::infrastructure::persistence::OidcSettingsRepository;
use crate::contexts::secrets::application::SecretStore;
use crate::contexts::secrets::infrastructure::SecretRepository;
use crate::errors::AppError;
use crate::integrations::oidc;

#[derive(Debug, Clone)]
pub struct EffectiveOidcConfig {
    pub auth: AuthConfig,
    pub client_secret: String,
    pub provisioning_mode: OidcProvisioningMode,
}

pub struct OidcSettingsService<S, R, Store>
where
    S: OidcSettingsRepository,
    R: SecretRepository,
    Store: SecretStore,
{
    repository: Arc<S>,
    secret_repository: Arc<R>,
    secret_store: Arc<Store>,
}

impl<S, R, Store> OidcSettingsService<S, R, Store>
where
    S: OidcSettingsRepository,
    R: SecretRepository,
    Store: SecretStore,
{
    pub fn new(repository: Arc<S>, secret_repository: Arc<R>, secret_store: Arc<Store>) -> Self {
        Self {
            repository,
            secret_repository,
            secret_store,
        }
    }

    pub async fn get_settings(&self) -> Result<Option<OidcSettings>, AppError> {
        self.repository.get_settings().await
    }

    pub async fn get_client_secret_state(
        &self,
        base_auth: &AuthConfig,
    ) -> Result<OidcClientSecretState, AppError> {
        let _ = base_auth;
        self.get_secret_engine_state().await
    }

    pub async fn get_effective(
        &self,
        base_auth: &AuthConfig,
    ) -> Result<EffectiveOidcConfig, AppError> {
        let settings = self.repository.get_settings().await?;

        let mut auth = base_auth.clone();
        let mut provisioning_mode = OidcProvisioningMode::Manual;
        if let Some(value) = settings.as_ref() {
            auth.oidc_enabled = value.enabled;
            auth.oidc_issuer_url = value.issuer_url.clone();
            auth.oidc_client_id = value.client_id.clone();
            auth.oidc_redirect_url = value.redirect_url.clone();
            auth.oidc_scopes = value.scopes.clone();
            auth.oidc_group_claim = value.group_claim.clone();
            provisioning_mode = value.provisioning_mode;
        }

        let client_secret = self.resolve_client_secret().await?.unwrap_or_default();
        if auth.oidc_enabled && client_secret.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC client secret is required".to_string(),
            ));
        }

        Ok(EffectiveOidcConfig {
            auth,
            client_secret,
            provisioning_mode,
        })
    }

    pub async fn upsert_settings(&self, settings: OidcSettings) -> Result<OidcSettings, AppError> {
        self.validate_for_persistence(&settings)?;
        self.repository.upsert_settings(settings).await
    }

    pub async fn test_connection(
        &self,
        auth: &AuthConfig,
        client_secret: &str,
    ) -> Result<(), AppError> {
        self.validate_auth_config(auth, client_secret)?;
        let _ = oidc::discover_client(auth, client_secret).await?;
        Ok(())
    }

    async fn resolve_client_secret(&self) -> Result<Option<String>, AppError> {
        let Some(binding) = self
            .secret_repository
            .get_usage_binding(
                OIDC_CLIENT_SECRET_CONSUMER_TYPE,
                oidc_client_secret_consumer_id()?,
                OIDC_CLIENT_SECRET_FIELD_KEY,
            )
            .await?
        else {
            return Ok(None);
        };

        let secret = match self.secret_store.get_secret(binding.secret_id).await {
            Ok(value) => value,
            Err(AppError::MissingSecret(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(normalize_optional(Some(secret.as_str())))
    }

    async fn get_secret_engine_state(&self) -> Result<OidcClientSecretState, AppError> {
        let Some(binding) = self
            .secret_repository
            .get_usage_binding(
                OIDC_CLIENT_SECRET_CONSUMER_TYPE,
                oidc_client_secret_consumer_id()?,
                OIDC_CLIENT_SECRET_FIELD_KEY,
            )
            .await?
        else {
            return Ok(OidcClientSecretState::default());
        };

        let has_value = match self.secret_store.get_secret(binding.secret_id).await {
            Ok(value) => normalize_optional(Some(value.as_str())).is_some(),
            Err(AppError::MissingSecret(_)) => false,
            Err(error) => return Err(error),
        };

        Ok(OidcClientSecretState {
            enabled: true,
            has_value,
        })
    }

    fn validate_for_persistence(&self, settings: &OidcSettings) -> Result<(), AppError> {
        if !settings.enabled {
            return Ok(());
        }

        let auth = AuthConfig {
            oidc_enabled: settings.enabled,
            oidc_issuer_url: settings.issuer_url.clone(),
            oidc_client_id: settings.client_id.clone(),
            oidc_redirect_url: settings.redirect_url.clone(),
            oidc_scopes: settings.scopes.clone(),
            oidc_group_claim: settings.group_claim.clone(),
            ..AuthConfig::default()
        };
        self.validate_non_secret_auth_fields(&auth)
    }

    fn validate_auth_config(&self, auth: &AuthConfig, client_secret: &str) -> Result<(), AppError> {
        self.validate_non_secret_auth_fields(auth)?;
        if client_secret.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC client secret is required".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_non_secret_auth_fields(&self, auth: &AuthConfig) -> Result<(), AppError> {
        if auth.oidc_issuer_url.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC issuer URL is required".to_string(),
            ));
        }
        if auth.oidc_client_id.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC client ID is required".to_string(),
            ));
        }
        if auth.oidc_redirect_url.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC redirect URL is required".to_string(),
            ));
        }
        if auth.oidc_group_claim.trim().is_empty() {
            return Err(AppError::Validation(
                "OIDC group claim is required".to_string(),
            ));
        }
        if auth.oidc_scopes.iter().all(|value| value.trim().is_empty()) {
            return Err(AppError::Validation(
                "At least one OIDC scope is required".to_string(),
            ));
        }
        Ok(())
    }
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    let trimmed = value.unwrap_or_default().trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
