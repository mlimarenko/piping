//! The transactional lock owner for a target set's JSON identity references.
//!
//! `playbook_job_template.inventory` and `playbook_schedule.targets` store the
//! target-set id inside JSON, so no foreign key guards those references. A
//! writer takes `FOR SHARE` on the set row in the transaction that writes the
//! reference; a delete takes `FOR UPDATE` and reads usage only after it holds
//! the lock. Both sides queue on the same row, so a committed reference always
//! appears in the delete's usage snapshot, and a deleted set always makes the
//! writer's existence check fail before it writes anything. The assignment FK
//! (`ON DELETE RESTRICT`) remains the independent backstop for its own rows.

use std::collections::BTreeSet;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::contexts::playbooks::domain::ScheduleTargets;
use crate::errors::{AppError, classify_sqlx_error_owned};

/// Take the delete lock on one set. `false` means the row is already gone.
pub(super) async fn lock_target_set_for_delete_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<bool, AppError> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM playbook_target_set WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(classify_sqlx_error_owned)?;
    Ok(locked.is_some())
}

/// Hold the reference lock on every set a write is about to reference. A
/// missing set is rejected here, before the caller writes its reference.
pub(super) async fn lock_target_sets_for_reference_tx(
    tx: &mut Transaction<'_, Postgres>,
    target_set_ids: &[Uuid],
) -> Result<(), AppError> {
    let ids: Vec<Uuid> = target_set_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let locked: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM playbook_target_set WHERE id = ANY($1) FOR SHARE")
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(classify_sqlx_error_owned)?;

    if let Some(missing) = ids.iter().find(|id| !locked.contains(id)) {
        return Err(AppError::Validation(format!(
            "Target set '{missing}' does not exist"
        )));
    }
    Ok(())
}

/// The target set a schedule's JSON selection references, if any.
pub(super) fn schedule_target_set_id(targets: &ScheduleTargets) -> Option<Uuid> {
    match targets {
        ScheduleTargets::Fixed { target_set_id, .. }
        | ScheduleTargets::Dynamic { target_set_id, .. } => *target_set_id,
    }
}
