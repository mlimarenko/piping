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
    fn relation(&self) -> (&'static str, &'static str) {
        match self {
            Self::Bay => ("dcim_modulebay_module_bay_types", "modulebay_id"),
            Self::ModuleType => ("dcim_moduletype_module_bay_types", "moduletype_id"),
            Self::Template => (
                "dcim_modulebaytemplate_module_bay_types",
                "modulebaytemplate_id",
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
    let (table, column) = owner.relation();
    // Identifiers are from the closed owner enum, never request input.
    sqlx::query(&format!("DELETE FROM {table} WHERE {column}=$1"))
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(classify_sqlx_error_owned)?;
    sqlx::query(&format!(
        "INSERT INTO {table} ({column},modulebaytype_id) SELECT $1,unnest($2::uuid[])"
    ))
    .bind(id)
    .bind(ids)
    .execute(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(())
}
