use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

use crate::contexts::access_control::infrastructure::persistence::repositories::{
    SettingValueRecord, SettingsRepository,
};
use crate::errors::AppError;

pub const SETTING_VALUE_MAX_BYTES: usize = 65_536;

static KEY_REGEX: OnceLock<Regex> = OnceLock::new();

fn key_regex() -> &'static Regex {
    KEY_REGEX.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "compile-time-constant pattern; a parse failure would be a programmer error"
        )]
        Regex::new(r"^[a-z][a-z0-9._-]*$").expect("settings key regex is valid")
    })
}

pub fn validate_setting_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() || key.len() > 200 {
        return Err(AppError::Validation(
            "Setting key must be between 1 and 200 characters".to_string(),
        ));
    }
    if !key_regex().is_match(key) {
        return Err(AppError::Validation(
            "Setting key must match ^[a-z][a-z0-9._-]*$".to_string(),
        ));
    }
    Ok(())
}

fn validate_value_size(value: &Value) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        AppError::Validation(format!("Failed to serialize setting value: {error}"))
    })?;
    if bytes.len() > SETTING_VALUE_MAX_BYTES {
        return Err(AppError::Validation(
            "setting value exceeds 64 KiB limit".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    User,
    Instance,
    Unset,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct EffectiveSetting {
    pub key: String,
    pub value: Option<Value>,
    pub source: SettingSource,
    pub user_value: Option<Value>,
    pub instance_value: Option<Value>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SettingOverride {
    pub key: String,
    pub value: Value,
    pub updated_at: DateTime<Utc>,
    pub updated_by_user_id: Option<Uuid>,
}

pub struct SettingsService {
    repo: Arc<dyn SettingsRepository>,
}

impl SettingsService {
    pub fn new(repo: Arc<dyn SettingsRepository>) -> Self {
        Self { repo }
    }

    pub async fn get_user_override(
        &self,
        user_id: Uuid,
        key: &str,
    ) -> Result<Option<Value>, AppError> {
        validate_setting_key(key)?;
        Ok(self
            .repo
            .get_user(user_id, key)
            .await?
            .map(|record| record.value))
    }

    pub async fn get_instance_default(&self, key: &str) -> Result<Option<Value>, AppError> {
        validate_setting_key(key)?;
        Ok(self
            .repo
            .get_instance(key)
            .await?
            .map(|record| record.value))
    }

    pub async fn upsert_user_override(
        &self,
        user_id: Uuid,
        key: &str,
        value: Value,
    ) -> Result<(), AppError> {
        validate_setting_key(key)?;
        validate_value_size(&value)?;
        self.repo.upsert_user(user_id, key, &value).await
    }

    pub async fn delete_user_override(&self, user_id: Uuid, key: &str) -> Result<bool, AppError> {
        validate_setting_key(key)?;
        self.repo.delete_user(user_id, key).await
    }

    pub async fn get_effective(
        &self,
        user_id: Uuid,
        key: &str,
    ) -> Result<EffectiveSetting, AppError> {
        validate_setting_key(key)?;
        let user = self.repo.get_user(user_id, key).await?;
        let instance = self.repo.get_instance(key).await?;
        Ok(Self::merge_setting(key.to_string(), user, instance))
    }

    pub async fn list_effective(
        &self,
        user_id: Uuid,
        requested_keys: Option<Vec<String>>,
    ) -> Result<Vec<EffectiveSetting>, AppError> {
        if let Some(keys) = requested_keys {
            let mut values = Vec::with_capacity(keys.len());
            for key in keys {
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                values.push(self.get_effective(user_id, key).await?);
            }
            values.sort_by(|a, b| a.key.cmp(&b.key));
            return Ok(values);
        }

        let user_values = self.repo.list_user(user_id).await?;
        let instance_values = self.repo.list_instance().await?;
        let keys: BTreeSet<String> = user_values
            .iter()
            .chain(instance_values.iter())
            .map(|record| record.key.clone())
            .collect();

        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            let user = user_values.iter().find(|record| record.key == key).cloned();
            let instance = instance_values
                .iter()
                .find(|record| record.key == key)
                .cloned();
            values.push(Self::merge_setting(key, user, instance));
        }
        Ok(values)
    }

    pub async fn list_instance_defaults(&self) -> Result<Vec<SettingOverride>, AppError> {
        Ok(self
            .repo
            .list_instance()
            .await?
            .into_iter()
            .map(SettingOverride::from)
            .collect())
    }

    pub async fn upsert_instance_default(
        &self,
        key: &str,
        value: Value,
        updated_by_user_id: Option<Uuid>,
    ) -> Result<SettingOverride, AppError> {
        validate_setting_key(key)?;
        validate_value_size(&value)?;
        Ok(self
            .repo
            .upsert_instance(key, &value, updated_by_user_id)
            .await?
            .into())
    }

    pub async fn delete_instance_default(&self, key: &str) -> Result<bool, AppError> {
        validate_setting_key(key)?;
        self.repo.delete_instance(key).await
    }

    fn merge_setting(
        key: String,
        user: Option<SettingValueRecord>,
        instance: Option<SettingValueRecord>,
    ) -> EffectiveSetting {
        match (user, instance) {
            (Some(user), instance) => EffectiveSetting {
                key,
                value: Some(user.value.clone()),
                source: SettingSource::User,
                user_value: Some(user.value),
                instance_value: instance.map(|record| record.value),
                updated_at: Some(user.updated_at),
            },
            (None, Some(instance)) => EffectiveSetting {
                key,
                value: Some(instance.value.clone()),
                source: SettingSource::Instance,
                user_value: None,
                instance_value: Some(instance.value),
                updated_at: Some(instance.updated_at),
            },
            (None, None) => EffectiveSetting {
                key,
                value: None,
                source: SettingSource::Unset,
                user_value: None,
                instance_value: None,
                updated_at: None,
            },
        }
    }
}

impl From<SettingValueRecord> for SettingOverride {
    fn from(record: SettingValueRecord) -> Self {
        Self {
            key: record.key,
            value: record.value,
            updated_at: record.updated_at,
            updated_by_user_id: record.updated_by_user_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_setting_keys() {
        assert!(validate_setting_key("app.theme").is_ok());
        assert!(validate_setting_key("connections.workspace.panel").is_ok());
        assert!(validate_setting_key("").is_err());
        assert!(validate_setting_key("App.theme").is_err());
        assert!(validate_setting_key("app/theme").is_err());
    }

    #[test]
    fn merge_prefers_user_over_instance() {
        let now = Utc::now();
        let setting = SettingsService::merge_setting(
            "ui.scale".to_string(),
            Some(SettingValueRecord {
                key: "ui.scale".to_string(),
                value: Value::from(125),
                updated_at: now,
                updated_by_user_id: None,
            }),
            Some(SettingValueRecord {
                key: "ui.scale".to_string(),
                value: Value::from(100),
                updated_at: now,
                updated_by_user_id: None,
            }),
        );

        assert_eq!(setting.source, SettingSource::User);
        assert_eq!(setting.value, Some(Value::from(125)));
        assert_eq!(setting.instance_value, Some(Value::from(100)));
    }
}
