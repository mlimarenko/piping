//! Provenance for source-pull records and native relationship units.

use std::collections::BTreeSet;

use chrono::Utc;
use sqlx::{Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use super::{
    ColumnValue, FieldKind, InventoryEntity, InventoryIdentity, PlannedRow, PreparedRow,
    find_binding,
};
use crate::contexts::sync::domain::{
    AppliedUnitState, CableEnd, MappingRef, NativeUnit, RelinquishReason, SourceEntity,
    SourceIdentity, UnitDigest, UnitMint, UnitOwnership,
};
use crate::contexts::sync::infrastructure::persistence::repositories::{
    MintOutcome, OwnershipRepository, SqlxOwnershipRepository,
};
use crate::errors::AppError;

pub(super) struct State {
    pub(super) digest: UnitDigest,
    pub(super) generation: i64,
}

enum Before {
    CreatedCandidate,
    Intact(UnitOwnership),
    Unproven,
}

pub(super) struct Observation {
    unit: NativeUnit,
    identity: SourceIdentity,
    before: Before,
}

pub(super) struct JunctionObservation {
    unit: NativeUnit,
    identity: SourceIdentity,
    local_column: &'static str,
    remote_column: &'static str,
    before: Before,
}

pub(super) struct TagObservation {
    unit: NativeUnit,
    identity: SourceIdentity,
    before: Before,
}

pub(super) struct CableObservation {
    unit: NativeUnit,
    identity: SourceIdentity,
    before: Before,
}

async fn cable_holder_covered(tx: &mut Transaction<'_, Postgres>) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sync_native_row_tables() r JOIN pg_class c ON c.relname=r.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_trigger t ON t.tgrelid=c.oid WHERE r.table_name='dcim_interface' AND t.tgname='sync_unit_lifecycle' AND t.tgenabled IN ('O','A') AND t.tgfoid='public.sync_native_unit_track_row()'::regprocedure)")
        .fetch_one(&mut **tx).await?)
}

pub(super) async fn cable_state(
    tx: &mut Transaction<'_, Postgres>,
    holder: Uuid,
) -> Result<Option<(Uuid, CableEnd, UnitDigest)>, AppError> {
    let row: Option<(Option<Uuid>, Option<String>, String)> = sqlx::query_as(
        "SELECT cable_id, cable_end::text, jsonb_build_array(cable_id,cable_end,cable_connector,cable_positions)::text FROM dcim_interface WHERE id=$1",
    ).bind(holder).fetch_optional(&mut **tx).await?;
    match row {
        None | Some((None, None, _)) => Ok(None),
        Some((Some(cable), Some(end), value)) => Ok(Some((
            cable,
            CableEnd::parse(&end)?,
            UnitDigest::of_v1(&[value.as_bytes()]),
        ))),
        Some(_) => Err(AppError::Conflict(
            "inventory_ownership:invalid_cable_attachment".into(),
        )),
    }
}

/// Observe the physical cable/end pair before any source write. A replacement
/// may remove only an intact edge owned by this exact source identity.
pub(super) async fn observe_cables(
    tx: &mut Transaction<'_, Postgres>,
    provider_id: Uuid,
    planned: &[PlannedRow],
) -> Result<Vec<CableObservation>, AppError> {
    use crate::contexts::inventory::domain::dcim::interface::InterfaceCableEnd;
    let repository = SqlxOwnershipRepository;
    let mut observations = Vec::new();
    for row in planned
        .iter()
        .filter(|row| row.entity == InventoryEntity::Interface)
    {
        let Some(requested) = super::interface_graph::attachment(row) else {
            if row.columns.iter().any(|column| {
                column.updatable
                    && matches!(
                        column.column,
                        "cable_id" | "cable_end" | "cable_connector" | "cable_positions"
                    )
            }) {
                return Err(AppError::Conflict(
                    "inventory_ownership:partial_cable_attachment".into(),
                ));
            }
            continue;
        };
        let wanted = match requested {
            (None, None) => None,
            (Some(cable), Some(InterfaceCableEnd::A)) => Some((cable, CableEnd::A)),
            (Some(cable), Some(InterfaceCableEnd::B)) => Some((cable, CableEnd::B)),
            _ => {
                return Err(AppError::Validation(
                    "inventory_ownership:invalid_cable_attachment".into(),
                ));
            }
        };
        let identity = SourceIdentity::new(
            provider_id,
            SourceEntity::new(&row.source_entity)?,
            &row.external_id,
        )?;
        let current = cable_state(tx, row.local_id).await?;
        let covered = cable_holder_covered(tx).await?;
        if let Some((cable, end, digest)) = &current
            && wanted != Some((*cable, *end))
        {
            let old = NativeUnit::cable_attachment(row.entity, row.local_id, *cable, *end)?;
            let generation = repository.lifecycle_generation(tx, &old).await?;
            let mut owner = repository.active_owner(tx, &old).await?.ok_or_else(|| {
                AppError::Conflict("inventory_ownership:unproven_cable_replacement".into())
            })?;
            if !covered
                || owner.provider_id != provider_id
                || owner.source_entity != row.source_entity
                || owner.external_id != row.external_id
                || generation != Some(owner.applied.lifecycle_generation)
                || owner.applied.digest != *digest
            {
                return Err(AppError::Conflict(
                    "inventory_ownership:unproven_cable_replacement".into(),
                ));
            }
            owner.mark_deleted(Utc::now())?;
            repository.retire(tx, &owner).await?;
        }
        let Some((cable, end)) = wanted else {
            continue;
        };
        let unit = NativeUnit::cable_attachment(row.entity, row.local_id, cable, end)?;
        let generation = repository.lifecycle_generation(tx, &unit).await?;
        let same = current
            .as_ref()
            .filter(|(id, side, _)| *id == cable && *side == end);
        let before = if let Some(mut owner) = repository.active_owner(tx, &unit).await? {
            if owner.provider_id != provider_id
                || owner.source_entity != row.source_entity
                || owner.external_id != row.external_id
            {
                return Err(AppError::Conflict(
                    "inventory_ownership:owner_conflict".into(),
                ));
            }
            if covered
                && same.is_some_and(|(_, _, digest)| *digest == owner.applied.digest)
                && generation == Some(owner.applied.lifecycle_generation)
            {
                Before::Intact(owner)
            } else {
                owner.relinquish(RelinquishReason::LocallyModified, Utc::now())?;
                repository.retire(tx, &owner).await?;
                Before::Unproven
            }
        } else if same.is_none() {
            Before::CreatedCandidate
        } else {
            Before::Unproven
        };
        observations.push(CableObservation {
            unit,
            identity,
            before,
        });
    }
    Ok(observations)
}

pub(super) async fn attest_cables(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    mapping: MappingRef,
    observations: Vec<CableObservation>,
) -> Result<(), AppError> {
    let repository = SqlxOwnershipRepository;
    for observation in observations {
        if matches!(&observation.before, Before::Unproven) {
            continue;
        }
        let Some((cable, end, digest)) = cable_state(tx, observation.unit.id()).await? else {
            continue;
        };
        if Some(cable) != observation.unit.member()
            || end.as_str() != observation.unit.member_token_column()
        {
            continue;
        }
        let Some(generation) = repository
            .lifecycle_generation(tx, &observation.unit)
            .await?
        else {
            continue;
        };
        if !cable_holder_covered(tx).await? {
            continue;
        }
        let now = Utc::now();
        let applied = AppliedUnitState {
            job_id,
            mapping,
            digest,
            lifecycle_generation: generation,
            applied_at: now,
        };
        match observation.before {
            Before::CreatedCandidate => {
                let claim = UnitOwnership::mint(UnitMint {
                    unit: observation.unit,
                    identity: observation.identity,
                    job_id,
                    mapping,
                    created_at: now,
                    applied,
                })?;
                match repository.mint(tx, &claim).await? {
                    MintOutcome::Minted
                    | MintOutcome::UncoveredNativeUnit
                    | MintOutcome::PreviouslyRetired => {}
                    MintOutcome::AlreadyOwned { .. } => {
                        return Err(AppError::Conflict(
                            "inventory_ownership:owner_conflict".into(),
                        ));
                    }
                }
            }
            Before::Intact(mut claim) => {
                claim.refresh(applied)?;
                repository.refresh(tx, &claim).await?;
            }
            Before::Unproven => {}
        }
    }
    Ok(())
}

pub(super) async fn tag_exists(
    tx: &mut Transaction<'_, Postgres>,
    unit: &NativeUnit,
) -> Result<bool, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM ");
    query
        .push(unit.table())
        .push(" WHERE id = ")
        .push_bind(unit.id())
        .push(" AND tags @> ARRAY[")
        .push_bind(unit.member_column().to_string())
        .push("]::text[])");
    Ok(query.build_query_scalar().fetch_one(&mut **tx).await?)
}

pub(super) async fn observe_tags(
    tx: &mut Transaction<'_, Postgres>,
    provider_id: Uuid,
    planned: &[PlannedRow],
) -> Result<Vec<TagObservation>, AppError> {
    let repository = SqlxOwnershipRepository;
    let mut observations = Vec::new();
    for row in planned {
        let Some(tags) = row.columns.iter().find_map(|column| match &column.value {
            ColumnValue::Tags(tags) if column.updatable => Some(tags),
            _ => None,
        }) else {
            continue;
        };
        let identity = SourceIdentity::new(
            provider_id,
            SourceEntity::new(&row.source_entity)?,
            &row.external_id,
        )?;
        for tag in tags {
            let tag = Uuid::parse_str(tag).map_err(|_| {
                AppError::Validation("inventory_ownership:invalid_tag_identity".into())
            })?;
            let unit = NativeUnit::tag_membership(row.entity, row.local_id, tag)?;
            let exists = tag_exists(tx, &unit).await?;
            let generation = repository.lifecycle_generation(tx, &unit).await?;
            let before = if let Some(mut owner) = repository.active_owner(tx, &unit).await? {
                if owner.provider_id != provider_id
                    || owner.source_entity != row.source_entity
                    || owner.external_id != row.external_id
                {
                    return Err(AppError::Conflict(
                        "inventory_ownership:owner_conflict".into(),
                    ));
                }
                if exists
                    && generation == Some(owner.applied.lifecycle_generation)
                    && owner.applied.digest == membership_digest(&unit)
                {
                    Before::Intact(owner)
                } else {
                    owner.relinquish(RelinquishReason::LocallyModified, Utc::now())?;
                    repository.retire(tx, &owner).await?;
                    Before::Unproven
                }
            } else if !exists {
                Before::CreatedCandidate
            } else {
                Before::Unproven
            };
            observations.push(TagObservation {
                unit,
                identity: identity.clone(),
                before,
            });
        }
    }
    Ok(observations)
}

pub(super) async fn attest_tags(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    mapping: MappingRef,
    observations: Vec<TagObservation>,
) -> Result<(), AppError> {
    let repository = SqlxOwnershipRepository;
    for observation in observations {
        if matches!(&observation.before, Before::Unproven)
            || !tag_exists(tx, &observation.unit).await?
        {
            continue;
        }
        let Some(generation) = repository
            .lifecycle_generation(tx, &observation.unit)
            .await?
        else {
            continue;
        };
        // The holder trigger is the lifecycle owner for its native tags array.
        let covered: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sync_native_row_tables() r JOIN pg_class c ON c.relname=r.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_trigger t ON t.tgrelid=c.oid WHERE r.table_name=$1 AND t.tgname='sync_unit_lifecycle' AND t.tgenabled IN ('O','A') AND t.tgfoid='public.sync_native_unit_track_row()'::regprocedure)")
            .bind(observation.unit.table()).fetch_one(&mut **tx).await?;
        if !covered {
            continue;
        }
        let now = Utc::now();
        let applied = AppliedUnitState {
            job_id,
            mapping,
            digest: membership_digest(&observation.unit),
            lifecycle_generation: generation,
            applied_at: now,
        };
        match observation.before {
            Before::CreatedCandidate => {
                let claim = UnitOwnership::mint(UnitMint {
                    unit: observation.unit,
                    identity: observation.identity,
                    job_id,
                    mapping,
                    created_at: now,
                    applied,
                })?;
                match repository.mint(tx, &claim).await? {
                    MintOutcome::Minted
                    | MintOutcome::UncoveredNativeUnit
                    | MintOutcome::PreviouslyRetired => {}
                    MintOutcome::AlreadyOwned { .. } => {
                        return Err(AppError::Conflict(
                            "inventory_ownership:owner_conflict".into(),
                        ));
                    }
                }
            }
            Before::Intact(mut claim) => {
                claim.refresh(applied)?;
                repository.refresh(tx, &claim).await?;
            }
            Before::Unproven => {}
        }
    }
    Ok(())
}

async fn junction_exists(
    tx: &mut Transaction<'_, Postgres>,
    unit: &NativeUnit,
    local: &str,
    remote: &str,
) -> Result<bool, AppError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM ");
    query
        .push(unit.table())
        .push(" WHERE ")
        .push(local)
        .push(" = ")
        .push_bind(unit.id())
        .push(" AND ")
        .push(remote)
        .push(" = ")
        .push_bind(unit.member().ok_or_else(|| {
            AppError::Internal("inventory_ownership:junction_member_missing".into())
        })?)
        .push(")");
    Ok(query.build_query_scalar().fetch_one(&mut **tx).await?)
}

/// The versioned digest of one membership unit: the native pair, never payload.
pub(super) fn membership_digest(unit: &NativeUnit) -> UnitDigest {
    let id = unit.id().to_string();
    let member = unit.member_column().to_string();
    UnitDigest::of_v1(&[unit.table().as_bytes(), id.as_bytes(), member.as_bytes()])
}

async fn junction_covered(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sync_native_junction_tables() j JOIN pg_class c ON c.relname=j.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_trigger t ON t.tgrelid=c.oid WHERE j.table_name=$1 AND t.tgname='sync_unit_lifecycle' AND t.tgenabled IN ('O','A') AND t.tgfoid='public.sync_native_unit_track_junction()'::regprocedure)")
        .bind(table).fetch_one(&mut **tx).await?)
}

pub(super) async fn observe_junctions(
    tx: &mut Transaction<'_, Postgres>,
    provider_id: Uuid,
    planned: &[PlannedRow],
) -> Result<Vec<JunctionObservation>, AppError> {
    let repository = SqlxOwnershipRepository;
    let mut observations = Vec::new();
    for row in planned {
        let identity = SourceIdentity::new(
            provider_id,
            SourceEntity::new(&row.source_entity)?,
            &row.external_id,
        )?;
        for junction in &row.junctions {
            if !junction_covered(tx, junction.table).await? {
                continue;
            }
            let canonical = crate::contexts::sync::domain::inventory_target_schema::ENTITIES
                .iter()
                .flat_map(|meta| meta.fields.iter())
                .find_map(|field| match field.kind {
                    FieldKind::Junction {
                        table,
                        local_column,
                        remote_column,
                        ..
                    }
                    | FieldKind::ContentTypeJunction {
                        table,
                        local_column,
                        remote_column,
                    } if table == junction.table => Some((local_column, remote_column)),
                    _ => None,
                });
            if canonical != Some((junction.local_column, junction.remote_column)) {
                continue;
            }
            let Some(field) = row.entity.field_iter().find(|field| match field.kind() {
                FieldKind::Junction {
                    table,
                    local_column,
                    remote_column,
                    ..
                }
                | FieldKind::ContentTypeJunction {
                    table,
                    local_column,
                    remote_column,
                } => {
                    table == junction.table
                        && local_column == junction.local_column
                        && remote_column == junction.remote_column
                }
                _ => false,
            }) else {
                continue;
            };
            for member in &junction.refs {
                let unit =
                    NativeUnit::junction_member(row.entity, field.name(), row.local_id, *member)?;
                let exists =
                    junction_exists(tx, &unit, junction.local_column, junction.remote_column)
                        .await?;
                let generation = repository.lifecycle_generation(tx, &unit).await?;
                let before = if let Some(mut owner) = repository.active_owner(tx, &unit).await? {
                    if owner.provider_id != provider_id
                        || owner.source_entity != row.source_entity
                        || owner.external_id != row.external_id
                    {
                        return Err(AppError::Conflict(
                            "inventory_ownership:owner_conflict".into(),
                        ));
                    }
                    if exists
                        && generation == Some(owner.applied.lifecycle_generation)
                        && owner.applied.digest == membership_digest(&unit)
                    {
                        Before::Intact(owner)
                    } else {
                        owner.relinquish(RelinquishReason::LocallyModified, Utc::now())?;
                        repository.retire(tx, &owner).await?;
                        Before::Unproven
                    }
                } else if !exists && generation.is_none() {
                    Before::CreatedCandidate
                } else {
                    Before::Unproven
                };
                observations.push(JunctionObservation {
                    unit,
                    identity: identity.clone(),
                    local_column: junction.local_column,
                    remote_column: junction.remote_column,
                    before,
                });
            }
        }
    }
    Ok(observations)
}

pub(super) async fn attest_junctions(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    mapping: MappingRef,
    observations: Vec<JunctionObservation>,
) -> Result<(), AppError> {
    let repository = SqlxOwnershipRepository;
    for observation in observations {
        if matches!(&observation.before, Before::Unproven)
            || !junction_exists(
                tx,
                &observation.unit,
                observation.local_column,
                observation.remote_column,
            )
            .await?
        {
            continue;
        }
        let Some(generation) = repository
            .lifecycle_generation(tx, &observation.unit)
            .await?
        else {
            continue;
        };
        let now = Utc::now();
        let applied = AppliedUnitState {
            job_id,
            mapping,
            digest: membership_digest(&observation.unit),
            lifecycle_generation: generation,
            applied_at: now,
        };
        match observation.before {
            Before::CreatedCandidate => {
                let claim = UnitOwnership::mint(UnitMint {
                    unit: observation.unit,
                    identity: observation.identity,
                    job_id,
                    mapping,
                    created_at: now,
                    applied,
                })?;
                match repository.mint(tx, &claim).await? {
                    MintOutcome::Minted
                    | MintOutcome::UncoveredNativeUnit
                    | MintOutcome::PreviouslyRetired => {}
                    MintOutcome::AlreadyOwned { .. } => {
                        return Err(AppError::Conflict(
                            "inventory_ownership:owner_conflict".into(),
                        ));
                    }
                }
            }
            Before::Intact(mut claim) => {
                claim.refresh(applied)?;
                repository.refresh(tx, &claim).await?;
            }
            Before::Unproven => {}
        }
    }
    Ok(())
}

/// Called after the native observation fence, which excludes competing native
/// writes. The proof and lifecycle tables also need a fence across providers.
pub(super) async fn lock_proofs(
    tx: &mut Transaction<'_, Postgres>,
    rows: &[PreparedRow],
) -> Result<(), AppError> {
    let mut tables = BTreeSet::new();
    let entities: BTreeSet<_> = rows.iter().map(|row| row.entity).collect();
    for entity in entities {
        let Some(target) = entity.table() else {
            continue;
        };
        for (table, _, _) in native_junctions(tx, target).await? {
            tables.insert(table);
        }
    }
    for table in tables {
        let mut query = QueryBuilder::<Postgres>::new("LOCK TABLE public.");
        query
            .push(table)
            .push(" IN SHARE ROW EXCLUSIVE MODE NOWAIT");
        query.build().execute(&mut **tx).await?;
    }
    sqlx::query("LOCK TABLE sync_inventory_ownership, sync_native_unit_lifecycle IN SHARE ROW EXCLUSIVE MODE NOWAIT")
        .execute(&mut **tx).await?;
    Ok(())
}

async fn native_junctions(
    tx: &mut Transaction<'_, Postgres>,
    target: &str,
) -> Result<Vec<(String, String, String)>, AppError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT j.table_name, a.attname::text, CASE WHEN a.attname=j.first_column THEN j.second_column ELSE j.first_column END FROM sync_native_junction_tables() j JOIN pg_class c ON c.relname=j.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_constraint fk ON fk.conrelid=c.oid AND fk.contype='f' AND array_length(fk.conkey,1)=1 JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=fk.conkey[1] AND a.attname IN (j.first_column,j.second_column) JOIN pg_class target ON target.oid=fk.confrelid JOIN pg_namespace target_ns ON target_ns.oid=target.relnamespace AND target_ns.nspname='public' WHERE target.relname=$1 ORDER BY 1,2,3",
    ).bind(target).fetch_all(&mut **tx).await?;
    for (table, local, remote) in &rows {
        for identifier in [table, local, remote] {
            if identifier.is_empty()
                || identifier.len() > 63
                || !identifier.as_bytes()[0].is_ascii_lowercase()
                || !identifier
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(AppError::Internal(
                    "inventory_ownership:invalid_native_identifier".into(),
                ));
            }
        }
    }
    Ok(rows)
}

fn eligible(entity: InventoryEntity) -> bool {
    entity.table().is_some()
}

/// Absence is not deletion authority. Retain native members not selected by a
/// source snapshot, including members that have no source provenance.
pub(super) async fn retain_memberships(
    tx: &mut Transaction<'_, Postgres>,
    planned: &mut [PlannedRow],
) -> Result<(), AppError> {
    for row in planned {
        for junction in &mut row.junctions {
            let mut query = QueryBuilder::<Postgres>::new("SELECT ");
            query
                .push(junction.remote_column)
                .push(" FROM ")
                .push(junction.table)
                .push(" WHERE ")
                .push(junction.local_column)
                .push(" = ")
                .push_bind(row.local_id);
            let current: Vec<Uuid> = query.build_query_scalar().fetch_all(&mut **tx).await?;
            junction.refs.extend(current);
            junction.refs.sort_unstable();
            junction.refs.dedup();
        }
        if let Some(tags) = row
            .columns
            .iter_mut()
            .find_map(|column| match &mut column.value {
                ColumnValue::Tags(tags) => Some(tags),
                _ => None,
            })
        {
            let table = row.entity.table().ok_or_else(|| {
                AppError::Internal("inventory_ownership:tag_holder_missing".into())
            })?;
            let mut query = QueryBuilder::<Postgres>::new("SELECT tags FROM ");
            query
                .push(table)
                .push(" WHERE id = ")
                .push_bind(row.local_id);
            let current: Option<Vec<String>> =
                query.build_query_scalar().fetch_optional(&mut **tx).await?;
            if let Some(current) = current {
                for tag in current {
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Hash the persisted row and lifecycle-catalogued native junctions.
/// Junction lifecycle history is included so remove/re-add with equal values is
/// still detectable. A catalogued junction without its lifecycle trigger makes
/// the record unproven rather than weakening the digest.
pub(super) async fn record_state(
    tx: &mut Transaction<'_, Postgres>,
    entity: InventoryEntity,
    unit: &NativeUnit,
) -> Result<Option<State>, AppError> {
    let repository = SqlxOwnershipRepository;
    let Some(generation) = repository.lifecycle_generation(tx, unit).await? else {
        return Ok(None);
    };
    let covered: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sync_native_row_tables() r JOIN pg_class c ON c.relname=r.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_trigger t ON t.tgrelid=c.oid WHERE r.table_name=$1 AND t.tgname='sync_unit_lifecycle' AND t.tgenabled IN ('O','A') AND t.tgfoid='public.sync_native_unit_track_row()'::regprocedure)",
    ).bind(unit.table()).fetch_one(&mut **tx).await?;
    if !covered {
        return Ok(None);
    }

    let mut query = QueryBuilder::<Postgres>::new("SELECT (to_jsonb(native_row) - ARRAY[");
    // Scope cache columns are reconstructed from the Device, not editable state.
    for (index, (column, _)) in entity.scope_cache_columns().iter().enumerate() {
        if index != 0 {
            query.push(",");
        }
        query.push_bind(*column);
    }
    query
        .push("]::text[])::text FROM ")
        .push(unit.table())
        .push(" native_row WHERE id=")
        .push_bind(unit.id());
    let row: Option<String> = query.build_query_scalar().fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut parts = vec![row.into_bytes()];
    let junctions = native_junctions(tx, unit.table()).await?;
    for field in entity.field_iter() {
        let relation = match field.kind() {
            FieldKind::Junction {
                table,
                local_column,
                ..
            }
            | FieldKind::ContentTypeJunction {
                table,
                local_column,
                ..
            } => Some((table, local_column)),
            _ => None,
        };
        if relation.is_some_and(|(table, local)| {
            !junctions
                .iter()
                .any(|(found_table, found_local, _)| found_table == table && found_local == local)
        }) {
            return Ok(None);
        }
    }
    for (table, local, remote) in junctions {
        let covered: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sync_native_junction_tables() j JOIN pg_class c ON c.relname=j.table_name JOIN pg_namespace n ON n.oid=c.relnamespace AND n.nspname='public' JOIN pg_trigger t ON t.tgrelid=c.oid WHERE j.table_name=$1 AND t.tgname='sync_unit_lifecycle' AND t.tgenabled IN ('O','A') AND t.tgfoid='public.sync_native_unit_track_junction()'::regprocedure)",
        ).bind(&table).fetch_one(&mut **tx).await?;
        if !covered {
            return Ok(None);
        }
        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT COALESCE(jsonb_agg(to_jsonb(member) ORDER BY to_jsonb(member)::text), '[]'::jsonb)::text FROM ",
        );
        query
            .push(&table)
            .push(" member WHERE ")
            .push(&local)
            .push(" = ")
            .push_bind(unit.id());
        let members: String = query.build_query_scalar().fetch_one(&mut **tx).await?;
        let position = if local < remote { 1_i32 } else { 2_i32 };
        let history: String = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_array(native_unit_key,generation) ORDER BY native_unit_key), '[]'::jsonb)::text FROM sync_native_unit_lifecycle WHERE native_table=$1 AND split_part(native_unit_key,'/',$2)=$3")
            .bind(&table).bind(position).bind(unit.id().to_string()).fetch_one(&mut **tx).await?;
        parts.push(table.as_bytes().to_vec());
        parts.push(members.into_bytes());
        parts.push(history.into_bytes());
    }
    let framed: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    Ok(Some(State {
        digest: UnitDigest::of_v1(&framed),
        generation,
    }))
}

pub(super) async fn observe(
    tx: &mut Transaction<'_, Postgres>,
    provider_id: Uuid,
    planned: &[PlannedRow],
) -> Result<Vec<Observation>, AppError> {
    let repository = SqlxOwnershipRepository;
    let mut observations = Vec::new();
    for row in planned.iter().filter(|row| eligible(row.entity)) {
        let unit = NativeUnit::record(row.entity, row.local_id)?;
        let identity = SourceIdentity::new(
            provider_id,
            SourceEntity::new(&row.source_entity)?,
            &row.external_id,
        )?;
        let binding = find_binding(
            tx,
            &InventoryIdentity {
                provider_id,
                entity: row.entity,
                external_id: row.external_id.clone(),
            },
        )
        .await?;
        let mut exists_query = QueryBuilder::<Postgres>::new("SELECT EXISTS (SELECT 1 FROM ");
        exists_query
            .push(unit.table())
            .push(" WHERE id=")
            .push_bind(unit.id())
            .push(")");
        let exists: bool = exists_query
            .build_query_scalar()
            .fetch_one(&mut **tx)
            .await?;
        let owner = repository.active_owner(tx, &unit).await?;
        let before = if let Some(mut owner) = owner {
            if owner.provider_id != provider_id
                || owner.source_entity != row.source_entity
                || owner.external_id != row.external_id
            {
                return Err(AppError::Conflict(
                    "inventory_ownership:owner_conflict".into(),
                ));
            }
            let current = record_state(tx, row.entity, &unit).await?;
            if current.as_ref().is_some_and(|current| {
                current.generation == owner.applied.lifecycle_generation
                    && current.digest == owner.applied.digest
            }) {
                Before::Intact(owner)
            } else {
                owner.relinquish(RelinquishReason::LocallyModified, Utc::now())?;
                repository.retire(tx, &owner).await?;
                Before::Unproven
            }
        } else if !exists
            && binding.is_none()
            && repository.lifecycle_generation(tx, &unit).await?.is_none()
        {
            Before::CreatedCandidate
        } else {
            Before::Unproven
        };
        observations.push(Observation {
            unit,
            identity,
            before,
        });
    }
    Ok(observations)
}

pub(super) async fn attest(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    mapping: MappingRef,
    observations: Vec<Observation>,
) -> Result<(), AppError> {
    let repository = SqlxOwnershipRepository;
    for observation in observations {
        if matches!(&observation.before, Before::Unproven) {
            continue;
        }
        let entity = InventoryEntity::from_source_key(observation.identity.entity.as_str())
            .ok_or_else(|| AppError::Internal("inventory_ownership:unknown_entity".into()))?;
        let Some(state) = record_state(tx, entity, &observation.unit).await? else {
            continue;
        };
        let now = Utc::now();
        let applied = AppliedUnitState {
            job_id,
            mapping,
            digest: state.digest,
            lifecycle_generation: state.generation,
            applied_at: now,
        };
        match observation.before {
            Before::CreatedCandidate => {
                let claim = UnitOwnership::mint(UnitMint {
                    unit: observation.unit,
                    identity: observation.identity,
                    job_id,
                    mapping,
                    created_at: now,
                    applied,
                })?;
                match repository.mint(tx, &claim).await? {
                    MintOutcome::Minted
                    | MintOutcome::UncoveredNativeUnit
                    | MintOutcome::PreviouslyRetired => {}
                    MintOutcome::AlreadyOwned { .. } => {
                        return Err(AppError::Conflict(
                            "inventory_ownership:owner_conflict".into(),
                        ));
                    }
                }
            }
            Before::Intact(mut claim) => {
                claim.refresh(applied)?;
                repository.refresh(tx, &claim).await?;
            }
            Before::Unproven => {}
        }
    }
    Ok(())
}
