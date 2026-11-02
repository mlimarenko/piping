use super::row_mappers::version_from_row;
use super::sql_strings::VERSION_COLUMNS;
use crate::contexts::playbooks::domain::{Playbook, PlaybookVersion};
use crate::errors::{AppError, classify_sqlx_error_owned};
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

pub(super) async fn latest_version_for(
    pool: &PgPool,
    playbook_id: Uuid,
) -> Result<Option<PlaybookVersion>, AppError> {
    let sql = format!(
        "SELECT {VERSION_COLUMNS} FROM playbook_version WHERE playbook_id = $1 ORDER BY version DESC LIMIT 1"
    );
    sqlx::query(&sql)
        .bind(playbook_id)
        .fetch_optional(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .as_ref()
        .map(version_from_row)
        .transpose()
}

pub(super) async fn current_version_for(
    pool: &PgPool,
    playbook: &Playbook,
) -> Result<Option<PlaybookVersion>, AppError> {
    if let Some(version_id) = playbook.current_version_id {
        return version_by_id_pool(pool, version_id).await;
    }
    latest_version_for(pool, playbook.id).await
}

pub(super) async fn version_by_id_pool(
    pool: &PgPool,
    version_id: Uuid,
) -> Result<Option<PlaybookVersion>, AppError> {
    let sql = format!("SELECT {VERSION_COLUMNS} FROM playbook_version WHERE id = $1");
    sqlx::query(&sql)
        .bind(version_id)
        .fetch_optional(pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .as_ref()
        .map(version_from_row)
        .transpose()
}

pub(super) async fn version_by_id(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    version_id: Uuid,
) -> Result<Option<PlaybookVersion>, AppError> {
    let sql = format!("SELECT {VERSION_COLUMNS} FROM playbook_version WHERE id = $1");
    sqlx::query(&sql)
        .bind(version_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?
        .as_ref()
        .map(version_from_row)
        .transpose()
}
