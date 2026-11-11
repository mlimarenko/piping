//! Persistence of source-ownership proofs and durable absence confirmation.
//!
//! Two tables have one owner each and this module is that owner:
//!
//! * `sync_inventory_ownership` — the typed claim of one native unit, with its
//!   immutable creation provenance and the native state the writer last applied.
//! * `sync_inventory_absence` — the durable two-observation confirmation of one
//!   provider, configured source generation and native unit.
//!
//! The counting rule of an absence is written once, in [`SqlxOwnershipRepository::observe_absent`],
//! so a retry inside one job is idempotent and a generation change restarts the
//! count instead of inheriting one. The claim transitions are owned by
//! `domain::ownership`; this module only persists the state that owner produced.
//!
//! All operations use the caller's transaction, including reads needed to decide
//! a mutation. This repository neither begins nor commits an independent write.
//! Nothing here deletes inventory. A mint requires an existing lifecycle entry for
//! the unit's native anchor, so a table without lifecycle coverage cannot be
//! proved and is reported as [`MintOutcome::UncoveredNativeUnit`] rather than
//! treated as safe.

use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::sync::domain::{
    AbsenceRecord, AppliedUnitState, MappingRef, NativeUnit, OwnershipState, RelinquishReason,
    SourceEntity, SourceGeneration, UnitDigest, UnitOwnership,
};
use crate::errors::AppError;

fn internal(message: String) -> AppError {
    AppError::Internal(format!("inventory_ownership: {message}"))
}

/// The ownership columns, in the order the row parsers expect.
const OWNERSHIP_COLUMNS: &str = "unit_kind, native_table, native_id, member_id, member_token,
    state, source_provider_id, source_entity, external_id,
    created_job_id, created_mapping_logical_id, created_mapping_version_id, created_at,
    last_applied_job_id, last_applied_mapping_logical_id, last_applied_mapping_version_id,
    last_applied_digest_version, last_applied_digest, last_applied_lifecycle_generation,
    last_applied_at, relinquished_at, relinquished_reason";

/// The absence columns, in the order the row parser expects.
const ABSENCE_COLUMNS: &str = "provider_id, unit_kind, native_table, native_id, member_id,
    member_token, source_generation, source_entity, external_id,
    observation_count, first_observed_at, first_observed_job_id,
    last_observed_at, last_observed_job_id, confirmed_at";

/// What happened when a writer asked to mint a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintOutcome {
    /// The claim is active and belongs to this provider.
    Minted,
    /// Another provider already owns the physical unit. No transfer happens.
    AlreadyOwned { provider_id: Uuid },
    /// The lifecycle entry is missing or no longer matches the supplied state.
    UncoveredNativeUnit,
    /// A previous claim was retired; creation provenance cannot be re-adopted.
    PreviouslyRetired,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait OwnershipRepository: Send + Sync {
    /// Mint a claim for a unit the caller created in its own transaction.
    ///
    /// A unit that already has an active owner is reported, never transferred,
    /// and a relinquished or deleted claim is never revived.
    /// The caller must establish native absence under its transaction fence before
    /// creation; a lifecycle entry alone is not creation provenance.
    /// The fence must also serialize ownership changes for this native unit,
    /// including retirement, until the transaction ends.
    async fn mint(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<MintOutcome, AppError>;

    /// The active claim of `provider_id` for `unit`, if this provider holds one.
    async fn find(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        unit: &NativeUnit,
    ) -> Result<Option<UnitOwnership>, AppError>;

    /// The active claim of any provider for `unit`.
    async fn active_owner(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        unit: &NativeUnit,
    ) -> Result<Option<UnitOwnership>, AppError>;

    /// Lock all active claims of one provider in native-identity order.
    ///
    /// The caller must hold the provider and native observation fences for the
    /// transaction; row locks alone do not prevent insertion of a new claim.
    async fn lock_active_for_provider(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
    ) -> Result<Vec<UnitOwnership>, AppError>;

    /// Persist newer applied state for an active claim of the same provider.
    async fn refresh(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<(), AppError>;

    /// Persist a relinquished or deleted claim.
    async fn retire(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<(), AppError>;

    /// The current native lifecycle generation of the unit's anchor, if the
    /// installed schema covers it.
    async fn lifecycle_generation(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        unit: &NativeUnit,
    ) -> Result<Option<i64>, AppError>;

    /// Record one completed-traversal absence observation and return the stored
    /// state after the transition.
    async fn observe_absent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &AbsenceRecord,
    ) -> Result<AbsenceRecord, AppError>;

    /// Clear the absence record of `unit`, because it was observed again.
    async fn clear_absent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        unit: &NativeUnit,
    ) -> Result<bool, AppError>;

    /// Confirmed absences of one provider under one configured source generation.
    async fn confirmed_absences(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        generation: &SourceGeneration,
        limit: i64,
    ) -> Result<Vec<AbsenceRecord>, AppError>;
}

#[derive(Clone, Copy, Default)]
pub struct SqlxOwnershipRepository;

fn stored_unit(row: &sqlx::postgres::PgRow) -> Result<NativeUnit, AppError> {
    let kind: String = row.try_get("unit_kind")?;
    let table: String = row.try_get("native_table")?;
    let member_token: String = row.try_get("member_token")?;
    NativeUnit::from_stored(
        &kind,
        &table,
        row.try_get("native_id")?,
        row.try_get("member_id")?,
        &member_token,
    )
}

fn stored_mapping(
    row: &sqlx::postgres::PgRow,
    logical: &str,
    version: &str,
) -> Result<MappingRef, AppError> {
    let logical_id: Uuid = row.try_get(logical)?;
    let version_id: i32 = row.try_get(version)?;
    if logical_id.is_nil() || version_id <= 0 {
        return Err(internal("stored mapping identity".to_string()));
    }
    Ok(MappingRef::new(logical_id, version_id))
}

fn stored_state(row: &sqlx::postgres::PgRow) -> Result<OwnershipState, AppError> {
    let state: String = row.try_get("state")?;
    OwnershipState::from_wire(&state)
        .ok_or_else(|| internal(format!("unknown ownership state {state}")))
}

fn stored_reason(row: &sqlx::postgres::PgRow) -> Result<Option<RelinquishReason>, AppError> {
    let reason: Option<String> = row.try_get("relinquished_reason")?;
    match reason {
        None => Ok(None),
        Some(value) => RelinquishReason::from_wire(&value)
            .map(Some)
            .ok_or_else(|| internal(format!("unknown relinquish reason {value}"))),
    }
}

fn stored_ownership(row: &sqlx::postgres::PgRow) -> Result<UnitOwnership, AppError> {
    let digest_version: i16 = row.try_get("last_applied_digest_version")?;
    let digest_hex: String = row.try_get("last_applied_digest")?;
    Ok(UnitOwnership {
        unit: stored_unit(row)?,
        provider_id: row.try_get("source_provider_id")?,
        source_entity: row.try_get("source_entity")?,
        external_id: row.try_get("external_id")?,
        state: stored_state(row)?,
        created_job_id: row.try_get("created_job_id")?,
        created_mapping: stored_mapping(
            row,
            "created_mapping_logical_id",
            "created_mapping_version_id",
        )?,
        created_at: row.try_get("created_at")?,
        applied: AppliedUnitState {
            job_id: row.try_get("last_applied_job_id")?,
            mapping: stored_mapping(
                row,
                "last_applied_mapping_logical_id",
                "last_applied_mapping_version_id",
            )?,
            digest: UnitDigest::from_stored(digest_version, &digest_hex)?,
            lifecycle_generation: row.try_get("last_applied_lifecycle_generation")?,
            applied_at: row.try_get("last_applied_at")?,
        },
        relinquished_at: row.try_get("relinquished_at")?,
        relinquished_reason: stored_reason(row)?,
    })
}

fn stored_absence(row: &sqlx::postgres::PgRow) -> Result<AbsenceRecord, AppError> {
    let generation: String = row.try_get("source_generation")?;
    let source_entity: String = row.try_get("source_entity")?;
    let record = AbsenceRecord {
        provider_id: row.try_get("provider_id")?,
        unit: stored_unit(row)?,
        generation: SourceGeneration::parse(&generation)?,
        source_entity: SourceEntity::new(&source_entity)?,
        external_id: row.try_get("external_id")?,
        observation_count: row.try_get("observation_count")?,
        first_observed_at: row.try_get("first_observed_at")?,
        first_observed_job_id: row.try_get("first_observed_job_id")?,
        last_observed_at: row.try_get("last_observed_at")?,
        last_observed_job_id: row.try_get("last_observed_job_id")?,
        confirmed_at: row.try_get("confirmed_at")?,
    };
    record.validate()?;
    Ok(record)
}

impl OwnershipRepository for SqlxOwnershipRepository {
    async fn lock_active_for_provider(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
    ) -> Result<Vec<UnitOwnership>, AppError> {
        if provider_id.is_nil() {
            return Err(AppError::Validation(
                "inventory_ownership:provider_id".into(),
            ));
        }
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(OWNERSHIP_COLUMNS).push(
            " FROM sync_inventory_ownership \
             WHERE source_provider_id = ",
        );
        query.push_bind(provider_id).push(
            " AND state = 'active' \
             ORDER BY unit_kind, native_table, native_id, member_id, member_token FOR UPDATE",
        );
        query
            .build()
            .fetch_all(&mut **tx)
            .await?
            .iter()
            .map(stored_ownership)
            .collect()
    }

    async fn mint(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<MintOutcome, AppError> {
        if ownership.state != OwnershipState::Active {
            return Err(AppError::Validation(
                "inventory_ownership: only an active claim can be minted".to_string(),
            ));
        }
        let (anchor_table, anchor_key) = ownership.unit.lifecycle_anchor()?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO sync_inventory_ownership (
                unit_kind, native_table, native_id, member_id, member_token,
                source_provider_id, source_entity, external_id,
                created_job_id, created_mapping_logical_id, created_mapping_version_id, created_at,
                last_applied_job_id, last_applied_mapping_logical_id, last_applied_mapping_version_id,
                last_applied_digest_version, last_applied_digest, last_applied_lifecycle_generation,
                last_applied_at
            )
            SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
            WHERE EXISTS (
                SELECT 1 FROM sync_native_unit_lifecycle
                WHERE native_table = $20 AND native_unit_key = $21 AND generation = $18
            ) AND NOT EXISTS (
                SELECT 1 FROM sync_inventory_ownership
                WHERE unit_kind = $1 AND native_table = $2 AND native_id = $3
                  AND member_id = $4 AND member_token = $5
            )
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(ownership.unit.kind().as_str())
        .bind(ownership.unit.table())
        .bind(ownership.unit.id())
        .bind(ownership.unit.member_column())
        .bind(ownership.unit.member_token_column())
        .bind(ownership.provider_id)
        .bind(&ownership.source_entity)
        .bind(&ownership.external_id)
        .bind(ownership.created_job_id)
        .bind(ownership.created_mapping.logical_id)
        .bind(ownership.created_mapping.version_id)
        .bind(ownership.created_at)
        .bind(ownership.applied.job_id)
        .bind(ownership.applied.mapping.logical_id)
        .bind(ownership.applied.mapping.version_id)
        .bind(ownership.applied.digest.version())
        .bind(ownership.applied.digest.as_str())
        .bind(ownership.applied.lifecycle_generation)
        .bind(ownership.applied.applied_at)
        .bind(anchor_table)
        .bind(&anchor_key)
        .execute(&mut **tx)
        .await?
            .rows_affected();
        if inserted == 1 {
            return Ok(MintOutcome::Minted);
        }
        match self.active_owner(tx, &ownership.unit).await? {
            Some(existing) => Ok(MintOutcome::AlreadyOwned {
                provider_id: existing.provider_id,
            }),
            None => {
                let retired: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM sync_inventory_ownership \
                     WHERE unit_kind = $1 AND native_table = $2 AND native_id = $3 \
                       AND member_id = $4 AND member_token = $5)",
                )
                .bind(ownership.unit.kind().as_str())
                .bind(ownership.unit.table())
                .bind(ownership.unit.id())
                .bind(ownership.unit.member_column())
                .bind(ownership.unit.member_token_column())
                .fetch_one(&mut **tx)
                .await?;
                Ok(if retired {
                    MintOutcome::PreviouslyRetired
                } else {
                    MintOutcome::UncoveredNativeUnit
                })
            }
        }
    }

    async fn find(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        unit: &NativeUnit,
    ) -> Result<Option<UnitOwnership>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(OWNERSHIP_COLUMNS).push(
            " FROM sync_inventory_ownership
            WHERE state = 'active'
              AND source_provider_id = $1
              AND unit_kind = $2 AND native_table = $3 AND native_id = $4
              AND member_id = $5 AND member_token = $6",
        );
        let row = query
            .build()
            .bind(provider_id)
            .bind(unit.kind().as_str())
            .bind(unit.table())
            .bind(unit.id())
            .bind(unit.member_column())
            .bind(unit.member_token_column())
            .fetch_optional(&mut **tx)
            .await?;
        row.as_ref().map(stored_ownership).transpose()
    }

    async fn active_owner(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        unit: &NativeUnit,
    ) -> Result<Option<UnitOwnership>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(OWNERSHIP_COLUMNS).push(
            " FROM sync_inventory_ownership
            WHERE state = 'active'
              AND unit_kind = $1 AND native_table = $2 AND native_id = $3
              AND member_id = $4 AND member_token = $5",
        );
        let row = query
            .build()
            .bind(unit.kind().as_str())
            .bind(unit.table())
            .bind(unit.id())
            .bind(unit.member_column())
            .bind(unit.member_token_column())
            .fetch_optional(&mut **tx)
            .await?;
        row.as_ref().map(stored_ownership).transpose()
    }

    async fn refresh(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<(), AppError> {
        let updated = sqlx::query(
            r#"
            UPDATE sync_inventory_ownership
            SET last_applied_job_id = $7,
                last_applied_mapping_logical_id = $8,
                last_applied_mapping_version_id = $9,
                last_applied_digest_version = $10,
                last_applied_digest = $11,
                last_applied_lifecycle_generation = $12,
                last_applied_at = $13
            WHERE state = 'active'
              AND source_provider_id = $1
              AND unit_kind = $2 AND native_table = $3 AND native_id = $4
              AND member_id = $5 AND member_token = $6
            "#,
        )
        .bind(ownership.provider_id)
        .bind(ownership.unit.kind().as_str())
        .bind(ownership.unit.table())
        .bind(ownership.unit.id())
        .bind(ownership.unit.member_column())
        .bind(ownership.unit.member_token_column())
        .bind(ownership.applied.job_id)
        .bind(ownership.applied.mapping.logical_id)
        .bind(ownership.applied.mapping.version_id)
        .bind(ownership.applied.digest.version())
        .bind(ownership.applied.digest.as_str())
        .bind(ownership.applied.lifecycle_generation)
        .bind(ownership.applied.applied_at)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(internal(
                "only an active claim of the same provider can be refreshed".to_string(),
            ));
        }
        Ok(())
    }

    async fn retire(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        ownership: &UnitOwnership,
    ) -> Result<(), AppError> {
        if ownership.state == OwnershipState::Active {
            return Err(AppError::Validation(
                "inventory_ownership: retiring a claim requires a retired state".to_string(),
            ));
        }
        let reason = ownership
            .relinquished_reason
            .ok_or_else(|| internal("retired claim without a reason".to_string()))?;
        let at = ownership
            .relinquished_at
            .ok_or_else(|| internal("retired claim without a time".to_string()))?;
        let updated = sqlx::query(
            r#"
            UPDATE sync_inventory_ownership
            SET state = $7, relinquished_at = $8, relinquished_reason = $9
            WHERE state = 'active'
              AND source_provider_id = $1
              AND unit_kind = $2 AND native_table = $3 AND native_id = $4
              AND member_id = $5 AND member_token = $6
            "#,
        )
        .bind(ownership.provider_id)
        .bind(ownership.unit.kind().as_str())
        .bind(ownership.unit.table())
        .bind(ownership.unit.id())
        .bind(ownership.unit.member_column())
        .bind(ownership.unit.member_token_column())
        .bind(ownership.state.as_str())
        .bind(at)
        .bind(reason.as_str())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(internal(
                "only an active claim of the same provider can be retired".to_string(),
            ));
        }
        Ok(())
    }

    async fn lifecycle_generation(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        unit: &NativeUnit,
    ) -> Result<Option<i64>, AppError> {
        let (table, key) = unit.lifecycle_anchor()?;
        let generation: Option<i64> = sqlx::query_scalar(
            "SELECT generation FROM sync_native_unit_lifecycle \
             WHERE native_table = $1 AND native_unit_key = $2",
        )
        .bind(table)
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;
        Ok(generation)
    }

    async fn observe_absent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &AbsenceRecord,
    ) -> Result<AbsenceRecord, AppError> {
        record.validate()?;
        if record.provider_id.is_nil() {
            return Err(AppError::Validation(
                "inventory_absence: provider is required".to_string(),
            ));
        }
        if record.first_observed_job_id.is_nil() || record.last_observed_job_id.is_nil() {
            return Err(AppError::Validation(
                "inventory_absence: an observation needs its job".to_string(),
            ));
        }
        let mut query = QueryBuilder::<Postgres>::new(
            r#"
            INSERT INTO sync_inventory_absence (
                provider_id, unit_kind, native_table, native_id, member_id, member_token,
                source_generation, source_entity, external_id,
                observation_count, first_observed_at, first_observed_job_id,
                last_observed_at, last_observed_job_id, confirmed_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, $10, $11, $10, $11, NULL)
            ON CONFLICT (provider_id, unit_kind, native_table, native_id, member_id, member_token)
            DO UPDATE
            SET source_entity = EXCLUDED.source_entity,
                external_id = EXCLUDED.external_id,
                source_generation = EXCLUDED.source_generation,
                first_observed_at = CASE
                    WHEN sync_inventory_absence.source_generation = EXCLUDED.source_generation
                        THEN sync_inventory_absence.first_observed_at
                    ELSE EXCLUDED.first_observed_at
                END,
                first_observed_job_id = CASE
                    WHEN sync_inventory_absence.source_generation = EXCLUDED.source_generation
                        THEN sync_inventory_absence.first_observed_job_id
                    ELSE EXCLUDED.first_observed_job_id
                END,
                observation_count = CASE
                    WHEN sync_inventory_absence.source_generation <> EXCLUDED.source_generation THEN 1
                    WHEN sync_inventory_absence.last_observed_job_id = EXCLUDED.last_observed_job_id
                        THEN sync_inventory_absence.observation_count
                    ELSE LEAST(sync_inventory_absence.observation_count + 1, 2)
                END,
                last_observed_at = EXCLUDED.last_observed_at,
                last_observed_job_id = EXCLUDED.last_observed_job_id,
                confirmed_at = CASE
                    WHEN sync_inventory_absence.source_generation <> EXCLUDED.source_generation THEN NULL
                    WHEN sync_inventory_absence.observation_count = 2
                        THEN sync_inventory_absence.confirmed_at
                    WHEN sync_inventory_absence.last_observed_job_id = EXCLUDED.last_observed_job_id
                        THEN sync_inventory_absence.confirmed_at
                    ELSE EXCLUDED.last_observed_at
                END
            RETURNING "#,
        );
        query.push(ABSENCE_COLUMNS);
        let row = query
            .build()
            .bind(record.provider_id)
            .bind(record.unit.kind().as_str())
            .bind(record.unit.table())
            .bind(record.unit.id())
            .bind(record.unit.member_column())
            .bind(record.unit.member_token_column())
            .bind(record.generation.as_str())
            .bind(record.source_entity.as_str())
            .bind(&record.external_id)
            .bind(record.first_observed_at)
            .bind(record.first_observed_job_id)
            .fetch_one(&mut **tx)
            .await?;
        stored_absence(&row)
    }

    async fn clear_absent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        unit: &NativeUnit,
    ) -> Result<bool, AppError> {
        let removed = sqlx::query(
            r#"
            DELETE FROM sync_inventory_absence
            WHERE provider_id = $1
              AND unit_kind = $2 AND native_table = $3 AND native_id = $4
              AND member_id = $5 AND member_token = $6
            "#,
        )
        .bind(provider_id)
        .bind(unit.kind().as_str())
        .bind(unit.table())
        .bind(unit.id())
        .bind(unit.member_column())
        .bind(unit.member_token_column())
        .execute(&mut **tx)
        .await?
        .rows_affected();
        Ok(removed > 0)
    }

    async fn confirmed_absences(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        provider_id: Uuid,
        generation: &SourceGeneration,
        limit: i64,
    ) -> Result<Vec<AbsenceRecord>, AppError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(ABSENCE_COLUMNS).push(
            " FROM sync_inventory_absence
            WHERE provider_id = $1
              AND source_generation = $2
              AND confirmed_at IS NOT NULL
            ORDER BY unit_kind, native_table, native_id, member_id
            LIMIT $3",
        );
        let rows = query
            .build()
            .bind(provider_id)
            .bind(generation.as_str())
            .bind(limit)
            .fetch_all(&mut **tx)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            out.push(stored_absence(row)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::domain::{InventoryEntity, SourceIdentity, UnitMint};
    use chrono::Utc;

    async fn insert_field_and_claim(tx: &mut Transaction<'_, Postgres>) -> UnitOwnership {
        let repository = SqlxOwnershipRepository;
        let id = Uuid::new_v4();
        let unit = NativeUnit::record(InventoryEntity::CustomField, id).expect("unit");
        let absent: bool =
            sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM custom_field WHERE id = $1)")
                .bind(id)
                .fetch_one(&mut **tx)
                .await
                .expect("absence");
        assert!(absent);
        sqlx::query("INSERT INTO custom_field (id, name) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("field_{}", id.simple()))
            .execute(&mut **tx)
            .await
            .expect("native field");
        let generation = repository
            .lifecycle_generation(tx, &unit)
            .await
            .expect("lifecycle")
            .expect("covered");
        let job = Uuid::new_v4();
        let mapping = MappingRef::new(Uuid::new_v4(), 1);
        let now = Utc::now();
        let identity = SourceIdentity::new(
            Uuid::new_v4(),
            SourceEntity::new("extras.custom-fields").expect("source entity"),
            "1",
        )
        .expect("identity");
        let ownership = UnitOwnership::mint(UnitMint {
            unit,
            identity,
            job_id: job,
            mapping,
            created_at: now,
            applied: AppliedUnitState {
                job_id: job,
                mapping,
                digest: UnitDigest::of_v1(&[b"test"]),
                lifecycle_generation: generation,
                applied_at: now,
            },
        })
        .expect("claim");
        assert_eq!(
            repository.mint(tx, &ownership).await.expect("mint"),
            MintOutcome::Minted
        );
        ownership
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn native_creation_and_proof_share_commit_and_rollback(pool: sqlx::PgPool) {
        let repository = SqlxOwnershipRepository;
        let mut tx = pool.begin().await.expect("transaction");
        let rolled_back = insert_field_and_claim(&mut tx).await;
        assert!(
            repository
                .find(&mut tx, rolled_back.provider_id, &rolled_back.unit)
                .await
                .expect("read own proof")
                .is_some()
        );
        tx.rollback().await.expect("rollback");
        let mut tx = pool.begin().await.expect("transaction");
        let native_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM custom_field WHERE id = $1)")
                .bind(rolled_back.unit.id())
                .fetch_one(&mut *tx)
                .await
                .expect("native visibility");
        assert!(!native_exists);
        assert!(
            repository
                .find(&mut tx, rolled_back.provider_id, &rolled_back.unit)
                .await
                .expect("proof visibility")
                .is_none()
        );
        let committed = insert_field_and_claim(&mut tx).await;
        let mut other = committed.clone();
        other.provider_id = Uuid::new_v4();
        assert_eq!(
            repository
                .mint(&mut tx, &other)
                .await
                .expect("owner conflict"),
            MintOutcome::AlreadyOwned {
                provider_id: committed.provider_id
            }
        );
        tx.commit().await.expect("commit");
        let mut tx = pool.begin().await.expect("transaction");
        assert!(
            repository
                .find(&mut tx, committed.provider_id, &committed.unit)
                .await
                .expect("committed proof")
                .is_some()
        );
        let native_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM custom_field WHERE id = $1)")
                .bind(committed.unit.id())
                .fetch_one(&mut *tx)
                .await
                .expect("committed native");
        assert!(native_exists);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn provider_claim_scan_excludes_other_owners_and_retired_units(pool: sqlx::PgPool) {
        let repository = SqlxOwnershipRepository;
        let mut tx = pool.begin().await.expect("transaction");
        let active = insert_field_and_claim(&mut tx).await;
        let mut retired = insert_field_and_claim(&mut tx).await;
        let other = insert_field_and_claim(&mut tx).await;
        retired
            .relinquish(RelinquishReason::LocallyModified, Utc::now())
            .expect("retire state");
        repository.retire(&mut tx, &retired).await.expect("retire");

        let claims = repository
            .lock_active_for_provider(&mut tx, active.provider_id)
            .await
            .expect("provider claims");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].unit, active.unit);
        assert_ne!(claims[0].provider_id, other.provider_id);
        assert!(
            repository
                .lock_active_for_provider(&mut tx, retired.provider_id)
                .await
                .expect("retired provider claims")
                .is_empty()
        );
        assert!(
            repository
                .lock_active_for_provider(&mut tx, Uuid::nil())
                .await
                .is_err()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn proof_changes_and_absence_observations_roll_back(pool: sqlx::PgPool) {
        let repository = SqlxOwnershipRepository;
        let mut tx = pool.begin().await.expect("transaction");
        let mut claim = insert_field_and_claim(&mut tx).await;
        tx.commit().await.expect("commit");
        let original = claim.applied.digest.clone();
        let now = Utc::now();
        let job = Uuid::new_v4();
        let record = AbsenceRecord {
            provider_id: claim.provider_id,
            unit: claim.unit.clone(),
            generation: SourceGeneration::parse(&"a".repeat(64)).expect("generation"),
            source_entity: SourceEntity::new("extras.custom-fields").expect("entity"),
            external_id: "1".into(),
            observation_count: 1,
            first_observed_at: now,
            first_observed_job_id: job,
            last_observed_at: now,
            last_observed_job_id: job,
            confirmed_at: None,
        };
        let mut tx = pool.begin().await.expect("transaction");
        claim.applied.digest = UnitDigest::of_v1(&[b"changed"]);
        repository.refresh(&mut tx, &claim).await.expect("refresh");
        assert_eq!(
            repository
                .find(&mut tx, claim.provider_id, &claim.unit)
                .await
                .expect("read")
                .expect("claim")
                .applied
                .digest,
            claim.applied.digest
        );
        repository
            .observe_absent(&mut tx, &record)
            .await
            .expect("observe");
        claim
            .relinquish(RelinquishReason::LocallyModified, now)
            .expect("retire state");
        repository.retire(&mut tx, &claim).await.expect("retire");
        assert!(repository.mint(&mut tx, &claim).await.is_err());
        tx.rollback().await.expect("rollback");
        let mut tx = pool.begin().await.expect("transaction");
        let restored = repository
            .find(&mut tx, claim.provider_id, &claim.unit)
            .await
            .expect("read")
            .expect("active claim");
        assert_eq!(restored.applied.digest, original);
        assert!(
            !repository
                .clear_absent(&mut tx, claim.provider_id, &claim.unit)
                .await
                .expect("absence rollback")
        );
        repository
            .observe_absent(&mut tx, &record)
            .await
            .expect("observe");
        tx.commit().await.expect("commit observation");
        let mut tx = pool.begin().await.expect("transaction");
        assert!(
            repository
                .clear_absent(&mut tx, claim.provider_id, &claim.unit)
                .await
                .expect("clear")
        );
        tx.rollback().await.expect("rollback clear");
        let mut tx = pool.begin().await.expect("transaction");
        assert!(
            repository
                .clear_absent(&mut tx, claim.provider_id, &claim.unit)
                .await
                .expect("clear restored observation")
        );
        let mut retired = restored.clone();
        retired
            .relinquish(RelinquishReason::LocallyModified, Utc::now())
            .expect("retire state");
        repository.retire(&mut tx, &retired).await.expect("retire");
        assert_eq!(
            repository
                .mint(&mut tx, &restored)
                .await
                .expect("reject retired"),
            MintOutcome::PreviouslyRetired
        );
    }
}
