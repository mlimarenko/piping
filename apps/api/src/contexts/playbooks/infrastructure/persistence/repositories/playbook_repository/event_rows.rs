use super::row_mappers::read;
use crate::contexts::playbooks::domain::{
    JobTokenAudience, PlaybookArtifact, PlaybookJobToken, PlaybookRunEvent, parse_job_token_status,
    parse_run_event_kind,
};
use crate::errors::AppError;
use sqlx::postgres::PgRow;
use uuid::Uuid;

pub(super) fn job_token_from_row(row: &PgRow) -> Result<PlaybookJobToken, AppError> {
    let status: String = read(row, "status")?;
    let metadata: serde_json::Value = read(row, "metadata")?;
    let audience = metadata
        .get("audience")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AppError::Database("Playbook job token has no audience binding".to_string())
        })
        .and_then(JobTokenAudience::parse)?;
    let owner_user_id = match metadata.get("owner_user_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .and_then(|raw| Uuid::parse_str(raw).ok())
                .ok_or_else(|| {
                    AppError::Database(
                        "Playbook job token has an invalid owner binding".to_string(),
                    )
                })?,
        ),
    };
    Ok(PlaybookJobToken {
        id: read(row, "id")?,
        run_id: read(row, "run_id")?,
        target_kind: read(row, "target_kind")?,
        target_id: read(row, "target_id")?,
        source_kind: read(row, "source_kind")?,
        source_id: read(row, "source_id")?,
        token_prefix: read(row, "token_prefix")?,
        status: parse_job_token_status(&status)?,
        scopes: read(row, "scopes")?,
        audience,
        owner_user_id,
        issued_at: read(row, "issued_at")?,
        expires_at: read(row, "expires_at")?,
        last_used_at: read(row, "last_used_at")?,
        revoked_at: read(row, "revoked_at")?,
        metadata,
    })
}

pub(super) fn artifact_from_row(row: &PgRow) -> Result<PlaybookArtifact, AppError> {
    Ok(PlaybookArtifact {
        id: read(row, "id")?,
        run_id: read(row, "run_id")?,
        artifact_type: read(row, "artifact_type")?,
        name: read(row, "name")?,
        content_type: read(row, "content_type")?,
        content_json: read(row, "content_json")?,
        content_text: read(row, "content_text")?,
        content_sha256: read(row, "content_sha256")?,
        content_size_bytes: read(row, "content_size_bytes")?,
        created_at: read(row, "created_at")?,
    })
}

pub(super) fn run_event_from_row(row: &PgRow) -> Result<PlaybookRunEvent, AppError> {
    let event_kind: String = read(row, "event_kind")?;
    Ok(PlaybookRunEvent {
        id: read(row, "id")?,
        run_group_id: read(row, "run_group_id")?,
        run_id: read(row, "run_id")?,
        host: read(row, "host")?,
        event_sequence: read(row, "event_sequence")?,
        event_kind: parse_run_event_kind(&event_kind)?,
        event_type: read(row, "event_type")?,
        message: read(row, "message")?,
        payload: read(row, "payload")?,
        created_at: read(row, "created_at")?,
    })
}
