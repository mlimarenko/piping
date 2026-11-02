use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::contexts::assistant::domain::{
    AssistantMessage, AssistantMessageRole, AssistantSession, AssistantSessionStatus,
    parse_assistant_message_role, parse_assistant_session_lifecycle,
    parse_assistant_session_status,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

#[derive(Debug, Clone)]
pub struct CreateAssistantSessionOutcome {
    pub session: Option<AssistantSession>,
    pub archived_session_id: Option<Uuid>,
    pub active_session_ids: Vec<Uuid>,
}

impl CreateAssistantSessionOutcome {
    pub fn created(session: AssistantSession, archived_session_id: Option<Uuid>) -> Self {
        Self {
            session: Some(session),
            archived_session_id,
            active_session_ids: Vec::new(),
        }
    }

    pub fn limit_reached(active_session_ids: Vec<Uuid>) -> Self {
        Self {
            session: None,
            archived_session_id: None,
            active_session_ids,
        }
    }
}

/// Caller-supplied inputs for a session-creation attempt that may rotate an
/// existing session. Borrowed so the trait stays allocation-free.
#[derive(Debug)]
pub struct AssistantSessionDraft<'a> {
    pub title: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub max_active_sessions: i32,
    pub archive_retention_days: i32,
}

#[async_trait]
pub trait AssistantRepository: Send + Sync {
    async fn purge_expired_archives(&self) -> Result<u64, AppError>;

    async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<AssistantSession>, AppError>;

    async fn get_session(&self, id: Uuid) -> Result<Option<AssistantSession>, AppError>;

    async fn list_messages(&self, session_id: Uuid) -> Result<Vec<AssistantMessage>, AppError>;

    async fn append_message(
        &self,
        session_id: Uuid,
        role: AssistantMessageRole,
        content_json: serde_json::Value,
    ) -> Result<AssistantMessage, AppError>;

    async fn create_session_with_rotation(
        &self,
        user_id: Uuid,
        draft: AssistantSessionDraft<'_>,
    ) -> Result<CreateAssistantSessionOutcome, AppError>;

    async fn update_session(
        &self,
        id: Uuid,
        title: Option<&str>,
        pinned: Option<bool>,
    ) -> Result<Option<AssistantSession>, AppError>;

    async fn update_session_status(
        &self,
        id: Uuid,
        status: AssistantSessionStatus,
        last_error: Option<&str>,
    ) -> Result<Option<AssistantSession>, AppError>;

    async fn claim_session_running(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<AssistantSession>, AppError>;

    async fn archive_session(
        &self,
        id: Uuid,
        user_id: Uuid,
        archive_retention_days: i32,
    ) -> Result<Option<AssistantSession>, AppError>;
}

#[derive(Clone)]
pub struct SqlxAssistantRepository {
    pool: PgPool,
}

impl SqlxAssistantRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn session_from_row(row: &PgRow) -> Result<AssistantSession, AppError> {
    let status: String = read(row, "status")?;
    let lifecycle: String = read(row, "lifecycle")?;
    Ok(AssistantSession {
        id: read(row, "id")?,
        user_id: read(row, "user_id")?,
        title: read(row, "title")?,
        status: parse_assistant_session_status(&status)?,
        lifecycle: parse_assistant_session_lifecycle(&lifecycle)?,
        pinned: read(row, "pinned")?,
        provider: read(row, "provider")?,
        model: read(row, "model")?,
        created_at: read(row, "created_at")?,
        updated_at: read(row, "updated_at")?,
        archived_at: read(row, "archived_at")?,
        purge_after: read(row, "purge_after")?,
        purged_at: read(row, "purged_at")?,
        last_error: read(row, "last_error")?,
        message_count: read(row, "message_count")?,
        metadata: read(row, "metadata")?,
    })
}

fn message_from_row(row: &PgRow) -> Result<AssistantMessage, AppError> {
    let role: String = read(row, "role")?;
    Ok(AssistantMessage {
        id: read(row, "id")?,
        session_id: read(row, "session_id")?,
        role: parse_assistant_message_role(&role)?,
        content_json: read(row, "content_json")?,
        created_at: read(row, "created_at")?,
    })
}

fn read<T>(row: &PgRow, column: &'static str) -> Result<T, AppError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column)
        .map_err(|error| AppError::Database(format!("Failed to read {column}: {error}")))
}

const SESSION_COLUMNS: &str = r#"
    id, user_id, title, status, lifecycle, pinned, provider, model,
    created_at, updated_at, archived_at, purge_after, purged_at,
    last_error, message_count, metadata
"#;

#[async_trait]
impl AssistantRepository for SqlxAssistantRepository {
    async fn purge_expired_archives(&self) -> Result<u64, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let rows = sqlx::query(
            r#"
            UPDATE assistant_sessions
            SET purged_at = NOW(),
                updated_at = NOW()
            WHERE lifecycle = 'archived'
              AND purge_after <= NOW()
              AND purged_at IS NULL
            RETURNING id
            "#,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let ids: Vec<Uuid> = rows
            .iter()
            .map(|row| read(row, "id"))
            .collect::<Result<_, _>>()?;
        if !ids.is_empty() {
            sqlx::query("DELETE FROM assistant_messages WHERE session_id = ANY($1)")
                .bind(&ids)
                .execute(&mut *tx)
                .await
                .map_err(classify_sqlx_error_owned)?;
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(ids.len() as u64)
    }

    async fn list_sessions_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<AssistantSession>, AppError> {
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM assistant_sessions WHERE user_id = $1 ORDER BY lifecycle ASC, updated_at DESC"
        );
        let rows = sqlx::query(&sql)
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;

        rows.iter().map(session_from_row).collect()
    }

    async fn get_session(&self, id: Uuid) -> Result<Option<AssistantSession>, AppError> {
        let sql = format!("SELECT {SESSION_COLUMNS} FROM assistant_sessions WHERE id = $1");
        sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(session_from_row)
            .transpose()
    }

    async fn list_messages(&self, session_id: Uuid) -> Result<Vec<AssistantMessage>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, session_id, role, content_json, created_at
            FROM assistant_messages
            WHERE session_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?;

        rows.iter().map(message_from_row).collect()
    }

    async fn append_message(
        &self,
        session_id: Uuid,
        role: AssistantMessageRole,
        content_json: serde_json::Value,
    ) -> Result<AssistantMessage, AppError> {
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let row = sqlx::query(
            r#"
            INSERT INTO assistant_messages (id, session_id, role, content_json)
            VALUES ($1, $2, $3, $4)
            RETURNING id, session_id, role, content_json, created_at
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(session_id)
        .bind(role.as_str())
        .bind(content_json)
        .fetch_one(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let message = message_from_row(&row)?;
        sqlx::query(
            r#"
            UPDATE assistant_sessions
            SET message_count = message_count + 1,
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .execute(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(message)
    }

    async fn create_session_with_rotation(
        &self,
        user_id: Uuid,
        draft: AssistantSessionDraft<'_>,
    ) -> Result<CreateAssistantSessionOutcome, AppError> {
        let AssistantSessionDraft {
            title,
            provider,
            model,
            max_active_sessions,
            archive_retention_days,
        } = draft;
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let active_rows = sqlx::query(
            r#"
            SELECT id, status, pinned
            FROM assistant_sessions
            WHERE user_id = $1 AND lifecycle = 'active'
            ORDER BY updated_at ASC, created_at ASC
            FOR UPDATE
            "#,
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        let mut archived_session_id = None;
        if active_rows.len() >= max_active_sessions.max(1) as usize {
            let mut active_ids = Vec::with_capacity(active_rows.len());
            let mut candidate_id = None;
            for row in &active_rows {
                let id: Uuid = read(row, "id")?;
                let status: String = read(row, "status")?;
                let pinned: bool = read(row, "pinned")?;
                active_ids.push(id);
                if candidate_id.is_none() && status != "running" && !pinned {
                    candidate_id = Some(id);
                }
            }

            let Some(candidate_id) = candidate_id else {
                tx.rollback().await.map_err(classify_sqlx_error_owned)?;
                return Ok(CreateAssistantSessionOutcome::limit_reached(active_ids));
            };

            sqlx::query(
                r#"
                UPDATE assistant_sessions
                SET lifecycle = 'archived',
                    archived_at = NOW(),
                    purge_after = NOW() + ($2::int * INTERVAL '1 day'),
                    updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(candidate_id)
            .bind(archive_retention_days.max(1))
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            archived_session_id = Some(candidate_id);
        }

        let sql = format!(
            r#"
            INSERT INTO assistant_sessions (id, user_id, title, provider, model)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING {SESSION_COLUMNS}
            "#
        );
        let row = sqlx::query(&sql)
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(title)
            .bind(provider)
            .bind(model)
            .fetch_one(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let session = session_from_row(&row)?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(CreateAssistantSessionOutcome::created(
            session,
            archived_session_id,
        ))
    }

    async fn update_session(
        &self,
        id: Uuid,
        title: Option<&str>,
        pinned: Option<bool>,
    ) -> Result<Option<AssistantSession>, AppError> {
        let sql = format!(
            r#"
            UPDATE assistant_sessions
            SET title = COALESCE($2, title),
                pinned = COALESCE($3, pinned),
                updated_at = NOW()
            WHERE id = $1 AND lifecycle = 'active'
            RETURNING {SESSION_COLUMNS}
            "#
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(title)
            .bind(pinned)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(session_from_row)
            .transpose()
    }

    async fn update_session_status(
        &self,
        id: Uuid,
        status: AssistantSessionStatus,
        last_error: Option<&str>,
    ) -> Result<Option<AssistantSession>, AppError> {
        let sql = format!(
            r#"
            UPDATE assistant_sessions
            SET status = $2,
                last_error = $3,
                updated_at = NOW()
            WHERE id = $1 AND lifecycle = 'active'
            RETURNING {SESSION_COLUMNS}
            "#
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(status.as_str())
            .bind(last_error)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(session_from_row)
            .transpose()
    }

    async fn claim_session_running(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<AssistantSession>, AppError> {
        let sql = format!(
            r#"
            UPDATE assistant_sessions
            SET status = 'running',
                last_error = NULL,
                updated_at = NOW()
            WHERE id = $1
              AND user_id = $2
              AND lifecycle = 'active'
              AND status <> 'running'
            RETURNING {SESSION_COLUMNS}
            "#
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(session_from_row)
            .transpose()
    }

    async fn archive_session(
        &self,
        id: Uuid,
        user_id: Uuid,
        archive_retention_days: i32,
    ) -> Result<Option<AssistantSession>, AppError> {
        let sql = format!(
            r#"
            UPDATE assistant_sessions
            SET lifecycle = 'archived',
                archived_at = COALESCE(archived_at, NOW()),
                purge_after = COALESCE(purge_after, NOW() + ($2::int * INTERVAL '1 day')),
                updated_at = NOW()
            WHERE id = $1
              AND user_id = $3
              AND lifecycle = 'active'
              AND status <> 'running'
            RETURNING {SESSION_COLUMNS}
            "#
        );
        sqlx::query(&sql)
            .bind(id)
            .bind(archive_retention_days.max(1))
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?
            .as_ref()
            .map(session_from_row)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::CreateAssistantSessionOutcome;
    use uuid::Uuid;

    #[test]
    fn limit_outcome_keeps_active_session_ids() {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let outcome = CreateAssistantSessionOutcome::limit_reached(vec![first, second]);
        assert!(outcome.session.is_none());
        assert_eq!(outcome.active_session_ids, vec![first, second]);
    }
}
