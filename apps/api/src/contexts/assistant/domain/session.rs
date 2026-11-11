use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantSessionStatus {
    Idle,
    Running,
    Error,
}

impl AssistantSessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for AssistantSessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantSessionLifecycle {
    Active,
    Archived,
}

impl AssistantSessionLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

impl fmt::Display for AssistantSessionLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssistantMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl AssistantMessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl fmt::Display for AssistantMessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub status: AssistantSessionStatus,
    pub lifecycle: AssistantSessionLifecycle,
    pub pinned: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub purge_after: Option<DateTime<Utc>>,
    pub purged_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub message_count: i32,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantMessage {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: AssistantMessageRole,
    pub content_json: Value,
    pub created_at: DateTime<Utc>,
}

pub fn parse_assistant_session_status(value: &str) -> Result<AssistantSessionStatus, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "idle" => Ok(AssistantSessionStatus::Idle),
        "running" => Ok(AssistantSessionStatus::Running),
        "error" => Ok(AssistantSessionStatus::Error),
        other => Err(AppError::Database(format!(
            "Unknown assistant session status: {other}"
        ))),
    }
}

pub fn parse_assistant_session_lifecycle(
    value: &str,
) -> Result<AssistantSessionLifecycle, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(AssistantSessionLifecycle::Active),
        "archived" => Ok(AssistantSessionLifecycle::Archived),
        other => Err(AppError::Database(format!(
            "Unknown assistant session lifecycle: {other}"
        ))),
    }
}

pub fn parse_assistant_message_role(value: &str) -> Result<AssistantMessageRole, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "system" => Ok(AssistantMessageRole::System),
        "user" => Ok(AssistantMessageRole::User),
        "assistant" => Ok(AssistantMessageRole::Assistant),
        "tool" => Ok(AssistantMessageRole::Tool),
        other => Err(AppError::Database(format!(
            "Unknown assistant message role: {other}"
        ))),
    }
}
