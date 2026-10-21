use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WizardProgress {
    pub id: Uuid,
    pub user_id: Uuid,
    pub wizard_type: String,
    pub current_step: Option<String>,
    pub step_data: Value,
    pub status: String,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStatus {
    Active,
    Completed,
    Dismissed,
}

impl std::fmt::Display for WizardStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => f.write_str("active"),
            Self::Completed => f.write_str("completed"),
            Self::Dismissed => f.write_str("dismissed"),
        }
    }
}

impl FromStr for WizardStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "dismissed" => Ok(Self::Dismissed),
            _ => Err(format!("Unsupported wizard status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardReadiness {
    pub has_hosts: bool,
    pub has_connection_configs: bool,
    pub user_has_credential_sets: bool,
    pub user_has_connected: bool,
    pub wizard_status: Option<String>,
    pub wizard_current_step: Option<String>,
    pub user_is_admin: bool,
}
