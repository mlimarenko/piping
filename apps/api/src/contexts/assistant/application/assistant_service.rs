use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::contexts::assistant::domain::{
    AssistantMessage, AssistantMessageRole, AssistantSession, AssistantSessionStatus,
};
use crate::contexts::assistant::infrastructure::persistence::{
    AssistantRepository,
    repositories::assistant_repository::{AssistantSessionDraft, CreateAssistantSessionOutcome},
};
use crate::errors::AppError;

pub const DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER: i32 = 5;
pub const MAX_ASSISTANT_MAX_SESSIONS_PER_USER: i32 = 20;
pub const DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS: i32 = 30;
pub const MAX_ASSISTANT_ARCHIVE_RETENTION_DAYS: i32 = 365;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantBootstrapConfig {
    pub instance_enabled: bool,
    pub provider: Option<String>,
    pub base_url_configured: bool,
    pub api_key_configured: bool,
    pub model: Option<String>,
    pub portal_context_enabled: bool,
    pub max_sessions_per_user: i32,
    pub archive_retention_days: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantBootstrapAccess {
    pub can_open: bool,
    pub can_execute: bool,
    pub can_update_preferences: bool,
    pub can_view_conversations: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantCreateSessionConflict {
    pub active_session_ids: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub enum AssistantCreateSessionResult {
    Created {
        session: Box<AssistantSession>,
        archived_session_id: Option<Uuid>,
    },
    LimitReached(AssistantCreateSessionConflict),
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AssistantSessionState {
    pub session: AssistantSession,
    pub messages: Vec<AssistantMessage>,
    pub transcript_available: bool,
    pub transcript_unavailable_reason: Option<String>,
}

/// Caller-supplied session-creation inputs before normalization and clamping.
#[derive(Debug, Clone)]
pub struct CreateAssistantSessionInput {
    pub title: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub max_active_sessions: i32,
    pub archive_retention_days: i32,
}

pub struct AssistantService<R>
where
    R: AssistantRepository,
{
    repo: Arc<R>,
}

impl<R> AssistantService<R>
where
    R: AssistantRepository,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub async fn purge_expired_archives(&self) -> Result<u64, AppError> {
        self.repo.purge_expired_archives().await
    }

    pub async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<AssistantSession>, AppError> {
        self.purge_expired_archives().await?;
        self.repo.list_sessions_for_user(user_id).await
    }

    pub async fn create_session(
        &self,
        user_id: Uuid,
        input: CreateAssistantSessionInput,
    ) -> Result<AssistantCreateSessionResult, AppError> {
        self.purge_expired_archives().await?;
        let title = normalize_session_title(input.title);
        let max_active_sessions = clamp_max_sessions(input.max_active_sessions);
        let archive_retention_days = clamp_archive_retention_days(input.archive_retention_days);
        let outcome = self
            .repo
            .create_session_with_rotation(
                user_id,
                AssistantSessionDraft {
                    title: &title,
                    provider: input.provider.as_deref(),
                    model: input.model.as_deref(),
                    max_active_sessions,
                    archive_retention_days,
                },
            )
            .await?;
        map_create_outcome(outcome)
    }

    pub async fn get_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<AssistantSession>, AppError> {
        self.purge_expired_archives().await?;
        self.repo.get_session(session_id).await
    }

    pub async fn get_session_state(
        &self,
        session_id: Uuid,
    ) -> Result<Option<AssistantSessionState>, AppError> {
        self.purge_expired_archives().await?;
        let Some(session) = self.repo.get_session(session_id).await? else {
            return Ok(None);
        };
        if session.purged_at.is_some() {
            return Ok(Some(AssistantSessionState {
                session,
                messages: Vec::new(),
                transcript_available: false,
                transcript_unavailable_reason: Some("expired".to_string()),
            }));
        }
        let messages = self.repo.list_messages(session_id).await?;
        Ok(Some(AssistantSessionState {
            session,
            messages,
            transcript_available: true,
            transcript_unavailable_reason: None,
        }))
    }

    pub async fn append_message(
        &self,
        session_id: Uuid,
        role: AssistantMessageRole,
        content_json: Value,
    ) -> Result<AssistantMessage, AppError> {
        self.repo
            .append_message(session_id, role, content_json)
            .await
    }

    pub async fn update_session_status(
        &self,
        session_id: Uuid,
        status: AssistantSessionStatus,
        last_error: Option<&str>,
    ) -> Result<Option<AssistantSession>, AppError> {
        self.repo
            .update_session_status(session_id, status, last_error)
            .await
    }

    pub async fn claim_session_running(
        &self,
        session_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<AssistantSession>, AppError> {
        self.repo.claim_session_running(session_id, user_id).await
    }

    pub async fn update_session(
        &self,
        session_id: Uuid,
        title: Option<String>,
        pinned: Option<bool>,
    ) -> Result<Option<AssistantSession>, AppError> {
        let title = title.map(|value| normalize_session_title(Some(value)));
        self.repo
            .update_session(session_id, title.as_deref(), pinned)
            .await
    }

    pub async fn archive_session(
        &self,
        session_id: Uuid,
        user_id: Uuid,
        archive_retention_days: i32,
    ) -> Result<Option<AssistantSession>, AppError> {
        self.repo
            .archive_session(
                session_id,
                user_id,
                clamp_archive_retention_days(archive_retention_days),
            )
            .await
    }
}

fn map_create_outcome(
    outcome: CreateAssistantSessionOutcome,
) -> Result<AssistantCreateSessionResult, AppError> {
    if let Some(session) = outcome.session {
        return Ok(AssistantCreateSessionResult::Created {
            session: Box::new(session),
            archived_session_id: outcome.archived_session_id,
        });
    }
    Ok(AssistantCreateSessionResult::LimitReached(
        AssistantCreateSessionConflict {
            active_session_ids: outcome.active_session_ids,
        },
    ))
}

pub fn clamp_max_sessions(value: i32) -> i32 {
    value.clamp(1, MAX_ASSISTANT_MAX_SESSIONS_PER_USER)
}

pub fn clamp_archive_retention_days(value: i32) -> i32 {
    value.clamp(1, MAX_ASSISTANT_ARCHIVE_RETENTION_DAYS)
}

pub fn normalize_session_title(value: Option<String>) -> String {
    let title = value
        .unwrap_or_else(|| "New assistant session".to_string())
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>();
    let title = if title.is_empty() {
        "New assistant session".to_string()
    } else {
        title
    };
    title.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS, DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER,
        MAX_ASSISTANT_ARCHIVE_RETENTION_DAYS, MAX_ASSISTANT_MAX_SESSIONS_PER_USER,
        clamp_archive_retention_days, clamp_max_sessions, normalize_session_title,
    };

    #[test]
    fn clamps_session_limits() {
        assert_eq!(clamp_max_sessions(0), 1);
        assert_eq!(
            clamp_max_sessions(DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER),
            DEFAULT_ASSISTANT_MAX_SESSIONS_PER_USER
        );
        assert_eq!(
            clamp_max_sessions(MAX_ASSISTANT_MAX_SESSIONS_PER_USER + 1),
            MAX_ASSISTANT_MAX_SESSIONS_PER_USER
        );
    }

    #[test]
    fn clamps_archive_retention() {
        assert_eq!(clamp_archive_retention_days(0), 1);
        assert_eq!(
            clamp_archive_retention_days(DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS),
            DEFAULT_ASSISTANT_ARCHIVE_RETENTION_DAYS
        );
        assert_eq!(
            clamp_archive_retention_days(MAX_ASSISTANT_ARCHIVE_RETENTION_DAYS + 1),
            MAX_ASSISTANT_ARCHIVE_RETENTION_DAYS
        );
    }

    #[test]
    fn normalizes_session_title() {
        assert_eq!(
            normalize_session_title(Some("  Ops\nchat  ".to_string())),
            "Opschat"
        );
        assert_eq!(
            normalize_session_title(Some("  ".to_string())),
            "New assistant session"
        );
        assert_eq!(normalize_session_title(None), "New assistant session");
    }
}
