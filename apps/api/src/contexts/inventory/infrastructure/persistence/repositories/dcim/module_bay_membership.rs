//! Atomic category replacement for the three native category owners.
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::errors::{AppError, classify_sqlx_error_owned};

pub enum ModuleBayCategoryOwner {
    Bay,
    ModuleType,
    Template,
}

impl ModuleBayCategoryOwner {
    fn statements(&self) -> (&'static str, &'static str) {
        match self {
            Self::Bay => (
                "DELETE FROM dcim_modulebay_module_bay_types WHERE modulebay_id=$1",
                "INSERT INTO dcim_modulebay_module_bay_types (modulebay_id,modulebaytype_id) SELECT $1,unnest($2::uuid[])",
            ),
            Self::ModuleType => (
                "DELETE FROM dcim_moduletype_module_bay_types WHERE moduletype_id=$1",
                "INSERT INTO dcim_moduletype_module_bay_types (moduletype_id,modulebaytype_id) SELECT $1,unnest($2::uuid[])",
            ),
            Self::Template => (
                "DELETE FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=$1",
                "INSERT INTO dcim_modulebaytemplate_module_bay_types (modulebaytemplate_id,modulebaytype_id) SELECT $1,unnest($2::uuid[])",
            ),
        }
    }
}

/// Caller must hold the owner lock before reading or changing its scalar state.
/// `None` preserves edges; a present empty array clears them; null is invalid.
pub async fn replace(
    tx: &mut Transaction<'_, Postgres>,
    owner: ModuleBayCategoryOwner,
    id: Uuid,
    patch: Option<&Option<Vec<Uuid>>>,
) -> Result<(), AppError> {
    let Some(patch) = patch else {
        return Ok(());
    };
    let ids = patch.as_ref().ok_or_else(|| {
        AppError::Validation("module_bay_types must be an array, not null".into())
    })?;
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        return Err(AppError::Validation(
            "module_bay_types must not contain duplicates".into(),
        ));
    }
    let found: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM dcim_modulebaytype WHERE id=ANY($1) ORDER BY id FOR KEY SHARE",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    if found.len() != ids.len() {
        return Err(AppError::Validation(
            "module_bay_types contains an unknown module bay type".into(),
        ));
    }
    let (delete, insert) = owner.statements();
    // Identifiers are from the closed owner enum, never request input.
    sqlx::query(delete)
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    sqlx::query(insert)
        .bind(id)
        .bind(ids)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}
