//! Official playbook catalog loader: filesystem snapshot -> database upsert.
//!
//! The catalog directory (baked into the image, optionally overlaid by a
//! git-refresh checkout) is read at startup and synced into the database in
//! one advisory-locked transaction. Playbooks execute from
//! `playbook_version.content` afterwards, so air-gapped installs never need
//! network access.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::manifest::{
    CatalogManifest, ManifestPlaybook, OFFICIAL_CATALOG_MANIFEST_FILE,
    OFFICIAL_CATALOG_MAX_CONTENT_BYTES, parse_manifest,
};
use crate::config::OfficialCatalogConfig;
use crate::errors::AppError;

/// Fixed historical id of the privileged system inventory playbook. It is NOT
/// derivable from the slug (predates uuidv5 derivation), so it is pinned here
/// and matched by the well-known slug below.
pub const SYSTEM_INVENTORY_PLAYBOOK_ID: &str = "00000000-0000-7000-8000-000000000247";
/// Well-known slug that maps to `SYSTEM_INVENTORY_PLAYBOOK_ID`.
pub const SYSTEM_INVENTORY_PLAYBOOK_SLUG: &str = "system-auto-update-inventory";
const SYSTEM_INVENTORY_TARGET_SET_ID: &str = "00000000-0000-7000-8000-000000000147";
const SYSTEM_INVENTORY_SERVICE_PRINCIPAL_ID: &str = "00000000-0000-7000-8000-000000000047";
/// Fixed id of the RBAC policy granting the system schedule principal its
/// fire-time rights (upserted by name; the fixed id keeps installs identical).
const SYSTEM_INVENTORY_PRINCIPAL_POLICY_ID: &str = "00000000-0000-7000-8000-000000000048";
const SYSTEM_INVENTORY_ASSIGNMENT_ID: &str = "00000000-0000-7000-8000-000000000547";
const SYSTEM_INVENTORY_SCHEDULE_ID: &str = "00000000-0000-7000-8000-000000000447";

/// UUIDv5 name prefix for official playbooks that do not pin an explicit id.
const OFFICIAL_PLAYBOOK_UUID_PREFIX: &str = "piping-official-playbook:";

/// Fallback catalog dir for local dev / `cargo test`, where the working
/// directory is `apps/api` while `ansible/playbooks` lives at the repo root.
/// Mirrors the migrations-dir resolution in `storage/database/migrations.rs`.
const LOCAL_DEV_FALLBACK_DIR: &str = "../../ansible/playbooks";

/// A loaded official catalog entry: manifest data plus resolved file content.
#[derive(Debug, Clone)]
pub struct OfficialCatalogEntry {
    pub manifest: ManifestPlaybook,
    /// Raw entrypoint file content, byte-exact (no normalization): the sha is
    /// the version dedup key, so baked and git reads must hash identically.
    pub content: String,
    pub content_sha256: String,
    pub vars_schema: Value,
}

/// The loaded official catalog, ready to sync into the database.
#[derive(Debug, Clone)]
pub struct OfficialCatalog {
    pub entries: Vec<OfficialCatalogEntry>,
}

/// One synced catalog entry (for startup logging).
#[derive(Debug, Clone)]
pub struct SyncedOfficialPlaybook {
    pub slug: String,
    pub playbook_id: Uuid,
    pub version_id: Uuid,
    pub system: bool,
}

/// Loads and validates the official catalog from `dir`. Fail-loud: startup
/// must not continue with a missing or malformed catalog.
pub fn load_official_catalog(dir: &Path) -> Result<OfficialCatalog, AppError> {
    let manifest_path = dir.join(OFFICIAL_CATALOG_MANIFEST_FILE);
    let raw = std::fs::read_to_string(&manifest_path).map_err(|error| {
        AppError::Config(format!(
            "Failed to read official catalog manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let manifest = parse_manifest(&raw)?;
    load_catalog_entries(dir, manifest)
}

/// Loads the catalog, degrading rather than preventing startup, and reports
/// every degradation it took.
///
/// There are three directories, not two. A broken `work_dir` overlay (e.g. an
/// interrupted git-refresh checkout whose manifest parses but whose content
/// files are missing) degrades to the baked snapshot; a baked snapshot without
/// a manifest degrades to `LOCAL_DEV_FALLBACK_DIR`, a path relative to the
/// working directory. That third step is the one worth shouting about: under
/// `cargo test` and in a source checkout it resolves to the repository's own
/// catalog and the load SUCCEEDS, so an image built without its baked catalog
/// starts up looking healthy while serving playbooks from a directory nobody
/// configured. Returning it as a warning is what makes that visible.
///
/// Returns the directory that actually loaded, the catalog, and every warning
/// the caller must log.
pub fn load_configured_catalog(
    config: &OfficialCatalogConfig,
) -> Result<(PathBuf, OfficialCatalog, Vec<String>), AppError> {
    let mut warnings = Vec::new();
    if !config.work_dir.trim().is_empty() {
        let work_dir = PathBuf::from(config.work_dir.trim());
        if work_dir.join(OFFICIAL_CATALOG_MANIFEST_FILE).is_file() {
            match load_official_catalog(&work_dir) {
                Ok(catalog) => return Ok((work_dir, catalog, warnings)),
                Err(error) => {
                    warnings.push(format!(
                        "Official catalog overlay {} is unusable, falling back to the baked snapshot: {error}",
                        work_dir.display()
                    ));
                }
            }
        }
    }
    let configured_baked_dir = PathBuf::from(&config.baked_dir);
    let baked_dir = if configured_baked_dir
        .join(OFFICIAL_CATALOG_MANIFEST_FILE)
        .is_file()
    {
        configured_baked_dir
    } else {
        warnings.push(format!(
            "Baked official catalog {} has no {}, loading the local-development catalog {} \
             resolved against the working directory instead",
            configured_baked_dir.display(),
            OFFICIAL_CATALOG_MANIFEST_FILE,
            LOCAL_DEV_FALLBACK_DIR
        ));
        PathBuf::from(LOCAL_DEV_FALLBACK_DIR)
    };
    let catalog = load_official_catalog(&baked_dir)?;
    Ok((baked_dir, catalog, warnings))
}

/// Reads a manifest-referenced file, rejecting symlinks and canonical-path
/// escapes. The manifest's lexical traversal validation cannot see the
/// filesystem; a git-refresh checkout of a compromised catalog commit could
/// otherwise smuggle a symlink to host files into `playbook_version.content`
/// (the sibling git-project walk in `project_bundle.rs` rejects symlinks the
/// same way).
fn read_catalog_file(dir: &Path, relative: &str) -> Result<Vec<u8>, AppError> {
    let full = dir.join(relative);
    let metadata = std::fs::symlink_metadata(&full).map_err(|error| {
        AppError::Config(format!(
            "Failed to read official catalog file {}: {error}",
            full.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(AppError::Validation(format!(
            "Official catalog file {} must be a regular file (symlinks are rejected)",
            full.display()
        )));
    }
    let canonical_dir = dir.canonicalize().map_err(|error| {
        AppError::Config(format!(
            "Failed to canonicalize official catalog dir {}: {error}",
            dir.display()
        ))
    })?;
    let canonical_full = full.canonicalize().map_err(|error| {
        AppError::Config(format!(
            "Failed to canonicalize official catalog file {}: {error}",
            full.display()
        ))
    })?;
    if !canonical_full.starts_with(&canonical_dir) {
        return Err(AppError::Validation(format!(
            "Official catalog file {} escapes the catalog directory",
            full.display()
        )));
    }
    std::fs::read(&canonical_full).map_err(|error| {
        AppError::Config(format!(
            "Failed to read official catalog file {}: {error}",
            canonical_full.display()
        ))
    })
}

fn load_catalog_entries(
    dir: &Path,
    manifest: CatalogManifest,
) -> Result<OfficialCatalog, AppError> {
    let mut entries = Vec::with_capacity(manifest.playbooks.len());
    for playbook in manifest.playbooks {
        let content_bytes = read_catalog_file(dir, &playbook.entrypoint_file)?;
        if content_bytes.is_empty() || content_bytes.len() > OFFICIAL_CATALOG_MAX_CONTENT_BYTES {
            return Err(AppError::Validation(format!(
                "Official catalog playbook `{}` content size {} must be within 1..={OFFICIAL_CATALOG_MAX_CONTENT_BYTES} bytes",
                playbook.slug,
                content_bytes.len()
            )));
        }
        // Hash the raw file bytes with NO normalization: this sha must be
        // byte-stable across baked and git-refresh reads so version dedup
        // never mints a spurious version on upgrade.
        let content_sha256 = hex_sha256(&content_bytes);
        let content = String::from_utf8(content_bytes).map_err(|error| {
            AppError::Validation(format!(
                "Official catalog playbook `{}` content must be UTF-8: {error}",
                playbook.slug
            ))
        })?;
        let vars_schema = match (&playbook.vars_schema_file, &playbook.vars_schema) {
            (Some(schema_file), None) => {
                let schema_bytes = read_catalog_file(dir, schema_file)?;
                let raw_schema = String::from_utf8(schema_bytes).map_err(|error| {
                    AppError::Validation(format!(
                        "Official catalog playbook `{}` vars schema must be UTF-8: {error}",
                        playbook.slug
                    ))
                })?;
                let schema: Value =
                    serde_json::from_str(&raw_schema).map_err(AppError::ValidationJson)?;
                if !schema.is_object() {
                    return Err(AppError::Validation(format!(
                        "Official catalog playbook `{}` vars schema file must contain a JSON object",
                        playbook.slug
                    )));
                }
                schema
            }
            (None, Some(inline)) => inline.clone(),
            // parse_manifest enforces exactly-one; keep the invariant fail-loud.
            _ => {
                return Err(AppError::Validation(format!(
                    "Official catalog playbook `{}` must declare exactly one of vars_schema_file or vars_schema",
                    playbook.slug
                )));
            }
        };
        entries.push(OfficialCatalogEntry {
            manifest: playbook,
            content,
            content_sha256,
            vars_schema,
        });
    }
    Ok(OfficialCatalog { entries })
}

/// Resolves the stable playbook id for a catalog entry.
///
/// - `system: true` entries pin the historical fixed UUID matched by the
///   well-known slug (`…0247` is NOT `uuidv5(slug)`; the satellite
///   assignment/schedule FKs on existing installs depend on it).
/// - Entries with an explicit manifest `id` use it verbatim.
/// - Everything else derives `uuidv5(NAMESPACE_URL,
///   "piping-official-playbook:" + slug)`.
pub fn resolve_playbook_id(entry: &ManifestPlaybook) -> Result<Uuid, AppError> {
    if entry.system {
        if entry.slug != SYSTEM_INVENTORY_PLAYBOOK_SLUG {
            return Err(AppError::Validation(format!(
                "Official catalog entry `{}` sets system:true but only `{SYSTEM_INVENTORY_PLAYBOOK_SLUG}` is a known system playbook",
                entry.slug
            )));
        }
        return Uuid::parse_str(SYSTEM_INVENTORY_PLAYBOOK_ID).map_err(AppError::ValidationUuid);
    }
    // The well-known slug is reserved: a `system: false` entry claiming it
    // would upsert onto the existing system playbook and silently downgrade
    // its source_kind, detaching the inventory-REST privilege.
    if entry.slug == SYSTEM_INVENTORY_PLAYBOOK_SLUG {
        return Err(AppError::Validation(format!(
            "Official catalog slug `{SYSTEM_INVENTORY_PLAYBOOK_SLUG}` is reserved for the system entry (set system: true)"
        )));
    }
    if let Some(id) = entry.id {
        return Ok(id);
    }
    let name = format!("{OFFICIAL_PLAYBOOK_UUID_PREFIX}{}", entry.slug);
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()))
}

/// Syncs the loaded catalog into the database: one transaction guarded by the
/// same advisory lock the legacy seed used, upserting playbooks and
/// dedup-inserting immutable versions. The `system: true` entry additionally
/// gets its built-in instance configuration (service principal, target set,
/// assignment, schedule) attached inside the same transaction.
pub async fn sync_official_catalog(
    db_pool: &PgPool,
    catalog: &OfficialCatalog,
) -> Result<Vec<SyncedOfficialPlaybook>, AppError> {
    let mut tx = db_pool.begin().await.map_err(AppError::DatabaseSource)?;
    sqlx::query("SELECT pg_advisory_xact_lock(7247000147)")
        .execute(&mut *tx)
        .await
        .map_err(AppError::DatabaseSource)?;

    let mut synced = Vec::with_capacity(catalog.entries.len());
    for entry in &catalog.entries {
        let playbook_id = resolve_playbook_id(&entry.manifest)?;
        let (playbook_id, version_id) = upsert_catalog_entry(&mut tx, playbook_id, entry).await?;
        if entry.manifest.system {
            ensure_system_inventory_instance_config(&mut tx, playbook_id, version_id, entry)
                .await?;
        }
        synced.push(SyncedOfficialPlaybook {
            slug: entry.manifest.slug.clone(),
            playbook_id,
            version_id,
            system: entry.manifest.system,
        });
    }

    tx.commit().await.map_err(AppError::DatabaseSource)?;
    Ok(synced)
}

/// Upserts one playbook row plus its immutable version, and points
/// `current_version_id` at the resolved version. Returns the effective
/// `(playbook_id, version_id)` (slug conflict keeps the pre-existing id).
async fn upsert_catalog_entry(
    tx: &mut Transaction<'_, Postgres>,
    playbook_id: Uuid,
    entry: &OfficialCatalogEntry,
) -> Result<(Uuid, Uuid), AppError> {
    let manifest = &entry.manifest;
    let source_kind = if manifest.system {
        "system"
    } else {
        "official"
    };
    let launch_options = manifest.launch_options.clone();
    let factory_metadata = json!({
        "system": manifest.system,
        "factory": true,
        "launch_options": launch_options.clone()
    });
    let playbook_metadata = json!({
        "system": manifest.system,
        "factory": true,
        "runtime": manifest.runtime,
        "category": manifest.category,
        "launch_options": launch_options
    });

    let playbook_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO playbook (
            id, slug, name, description, status, runtime, risk, category,
            metadata, source_kind, vars_schema, connection_default,
            inventory_rest_enabled
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (slug) DO UPDATE
        SET name = EXCLUDED.name,
            description = EXCLUDED.description,
            status = EXCLUDED.status,
            runtime = EXCLUDED.runtime,
            risk = EXCLUDED.risk,
            category = EXCLUDED.category,
            metadata = playbook.metadata || EXCLUDED.metadata,
            source_kind = EXCLUDED.source_kind,
            vars_schema = EXCLUDED.vars_schema,
            connection_default = EXCLUDED.connection_default,
            inventory_rest_enabled = EXCLUDED.inventory_rest_enabled,
            updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(playbook_id)
    .bind(&manifest.slug)
    .bind(&manifest.name)
    .bind(&manifest.description)
    .bind(&manifest.status)
    .bind(&manifest.runtime)
    .bind(&manifest.risk)
    .bind(&manifest.category)
    .bind(&playbook_metadata)
    .bind(source_kind)
    .bind(&entry.vars_schema)
    .bind(&manifest.connection_default)
    .bind(manifest.inventory_rest_enabled)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    // Versions are immutable: SELECT-before-INSERT on the dedup predicate
    // (content sha + factory metadata + timeout). A matching version — e.g.
    // the pre-existing db-seeded row on upgraded installs — is reused, never
    // bumped. Do NOT route this through the generic create_playbook_version,
    // which always mints a new version.
    //
    // `@>` is containment, not equality: adding a NEW launch_options key to
    // the manifest makes $3 a non-subset of every stored row and mints one
    // spurious version per install on next boot. The unit test locking the
    // launch_options key set exists to make that a conscious decision.
    let mut version_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM playbook_version
        WHERE playbook_id = $1
          AND content_sha256 = $2
          AND metadata @> $3::jsonb
          AND timeout_seconds = $4
        ORDER BY version DESC
        LIMIT 1
        "#,
    )
    .bind(playbook_id)
    .bind(&entry.content_sha256)
    .bind(&factory_metadata)
    .bind(manifest.timeout_seconds)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    if version_id.is_none() {
        version_id = Some(
            sqlx::query_scalar(
                r#"
                INSERT INTO playbook_version (
                    playbook_id, version, language, runtime, entrypoint,
                    content, content_sha256, input_schema, output_schema,
                    token_policy, timeout_seconds, changelog, metadata
                )
                SELECT
                    $1,
                    COALESCE(MAX(version), 0) + 1,
                    $2,
                    $3,
                    NULL,
                    $4,
                    $5,
                    $6,
                    $7,
                    $8,
                    $9,
                    'Sync official playbook catalog content.',
                    $10
                FROM playbook_version
                WHERE playbook_id = $1
                RETURNING id
                "#,
            )
            .bind(playbook_id)
            .bind(&manifest.language)
            .bind(&manifest.runtime)
            .bind(&entry.content)
            .bind(&entry.content_sha256)
            .bind(&manifest.input_schema)
            .bind(&manifest.output_schema)
            .bind(&manifest.token_policy)
            .bind(manifest.timeout_seconds)
            .bind(&factory_metadata)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::DatabaseSource)?,
        );
    }
    let version_id = version_id.ok_or_else(|| {
        AppError::Internal(format!(
            "Official catalog version sync failed for `{}`",
            manifest.slug
        ))
    })?;

    sqlx::query(
        r#"
        UPDATE playbook
        SET current_version_id = $2,
            updated_at = NOW()
        WHERE id = $1 AND current_version_id IS DISTINCT FROM $2
        "#,
    )
    .bind(playbook_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    Ok((playbook_id, version_id))
}

/// Attaches the built-in instance configuration of the system inventory
/// playbook: service principal, dynamic target set, global assignment, and
/// the disabled daily schedule. Instance config deliberately stays in Rust —
/// the manifest owns content and capability flags only — but
/// `launch_options` is taken FROM the manifest entry so there is a single
/// source of truth for it.
///
/// FK-correct order (do not reorder): the service principal and target set
/// are created before the schedule/assignment rows and the
/// `default_target_set_id` pointer that reference them; the version id passed
/// in already exists.
async fn ensure_system_inventory_instance_config(
    tx: &mut Transaction<'_, Postgres>,
    playbook_id: Uuid,
    version_id: Uuid,
    entry: &OfficialCatalogEntry,
) -> Result<(), AppError> {
    let target_set_id = Uuid::parse_str(SYSTEM_INVENTORY_TARGET_SET_ID)?;
    let service_principal_id = Uuid::parse_str(SYSTEM_INVENTORY_SERVICE_PRINCIPAL_ID)?;
    let assignment_id = Uuid::parse_str(SYSTEM_INVENTORY_ASSIGNMENT_ID)?;
    let schedule_id = Uuid::parse_str(SYSTEM_INVENTORY_SCHEDULE_ID)?;

    let launch_options = entry.manifest.launch_options.clone();
    let factory_metadata = json!({
        "system": true,
        "factory": true,
        "launch_options": launch_options.clone()
    });
    let assignment_default_metadata = json!({
        "launch_options": launch_options
    });
    let schedule_metadata = json!({
        "system": true,
        "factory": true,
        "disabled_reason": "service_principal_credentials_not_configured"
    });
    let target_selector = system_inventory_target_selector();

    let service_principal_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO playbook_service_principal (
            id, name, slug, description, status, metadata
        )
        VALUES (
            $1,
            'system-auto-update-inventory',
            'system-auto-update-inventory',
            'Built-in non-revocable principal for the auto-update inventory schedule.',
            'active',
            $2
        )
        ON CONFLICT (slug) DO UPDATE
        SET name = EXCLUDED.name,
            description = EXCLUDED.description,
            status = CASE
                WHEN playbook_service_principal.status = 'revoked' THEN playbook_service_principal.status
                ELSE EXCLUDED.status
            END,
            metadata = playbook_service_principal.metadata || EXCLUDED.metadata,
            updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(service_principal_id)
    .bind(json!({"system": true, "non_revocable": true}))
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    // A service principal's permissions come from RBAC policies (subject
    // `service_principal:{id}`) — the same mechanism as users; there is no
    // side-channel scopes field. Seed the least-privilege policy the schedule
    // worker's fire-time check (`authorize_scheduled_run`) requires: Execute on
    // THIS playbook, Read on THIS target set, Create on the `scheduled` run
    // group literal. Without it the built-in schedule would fail default-deny
    // the moment an operator enables it.
    sqlx::query(
        r#"
        INSERT INTO rbac_policy (
            id, name, description, effect, actions, resource_types,
            constraints, subjects, resources, enabled
        )
        VALUES (
            $1,
            'system-auto-update-inventory-principal',
            'Least-privilege grants for the built-in inventory auto-update schedule principal.',
            'allow',
            '["EXECUTE", "READ", "CREATE"]'::jsonb,
            '["PLAYBOOK", "PLAYBOOK_TARGET_SET", "PLAYBOOK_RUN_GROUP"]'::jsonb,
            NULL,
            $2,
            $3,
            true
        )
        ON CONFLICT (name) DO UPDATE
        SET subjects = EXCLUDED.subjects,
            resources = EXCLUDED.resources,
            actions = EXCLUDED.actions,
            resource_types = EXCLUDED.resource_types,
            enabled = true
        "#,
    )
    .bind(Uuid::parse_str(SYSTEM_INVENTORY_PRINCIPAL_POLICY_ID)?)
    .bind(json!([format!("service_principal:{service_principal_id}")]))
    // Bare resource ids, not `type:id` canonical forms: the fire-time check
    // passes bare ids, and `matches_resource_pattern` compares a `type:`-prefixed
    // pattern against the raw id (no canonicalization), so a prefixed pattern
    // would never match. Type-scoping is already enforced per check by the
    // policy's `resource_types` list.
    .bind(json!([
        playbook_id.to_string(),
        target_set_id.to_string(),
        "scheduled",
    ]))
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    sqlx::query(
        r#"
        INSERT INTO playbook_target_set (
            id, name, mode, entity_kind, selector
        )
        VALUES ($1, 'All SSH hosts', 'dynamic', 'connection_target', $2)
        ON CONFLICT (id) DO UPDATE
        SET name = EXCLUDED.name,
            mode = EXCLUDED.mode,
            entity_kind = EXCLUDED.entity_kind,
            selector = EXCLUDED.selector,
            updated_at = NOW()
        "#,
    )
    .bind(target_set_id)
    .bind(&target_selector)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    sqlx::query(
        r#"
        UPDATE playbook
        SET default_target_set_id = COALESCE(default_target_set_id, $2),
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(playbook_id)
    .bind(target_set_id)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    sqlx::query(
        r#"
        INSERT INTO playbook_assignment (
            id, playbook_id, target_set_id, default_version_id, name,
            scope_kind, scope_id, priority, enabled, visible, executable,
            quick_action_enabled, default_input, default_metadata, metadata
        )
        VALUES (
            $1, $2, $3, $4, 'Run inventory auto-update on SSH hosts',
            'global', NULL, 0, true, true, true, false, '{}'::jsonb,
            $5, $6
        )
        ON CONFLICT (id) DO UPDATE
        SET playbook_id = EXCLUDED.playbook_id,
            target_set_id = EXCLUDED.target_set_id,
            default_version_id = EXCLUDED.default_version_id,
            name = EXCLUDED.name,
            scope_kind = EXCLUDED.scope_kind,
            scope_id = EXCLUDED.scope_id,
            priority = EXCLUDED.priority,
            enabled = EXCLUDED.enabled,
            visible = EXCLUDED.visible,
            executable = EXCLUDED.executable,
            quick_action_enabled = EXCLUDED.quick_action_enabled,
            metadata = playbook_assignment.metadata || EXCLUDED.metadata,
            updated_at = NOW()
        "#,
    )
    .bind(assignment_id)
    .bind(playbook_id)
    .bind(target_set_id)
    .bind(version_id)
    .bind(&assignment_default_metadata)
    .bind(&factory_metadata)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    // Existing assignments retain customer metadata; only manifest-owned
    // launch option keys are refreshed, including the fixed bootstrap row.
    sqlx::query(
        r#"
        UPDATE playbook_assignment
        SET default_metadata = jsonb_set(
                COALESCE(default_metadata, '{}'::jsonb),
                '{launch_options}',
                COALESCE(default_metadata -> 'launch_options', '{}'::jsonb)
                    || ($2::jsonb -> 'launch_options'),
                true
            ),
            updated_at = NOW()
        WHERE playbook_id = $1
        "#,
    )
    .bind(playbook_id)
    .bind(&assignment_default_metadata)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    sqlx::query(
        r#"
        INSERT INTO playbook_schedule (
            id, playbook_id, target_set_id, service_principal_id, name,
            cron_expression, timezone, misfire_policy, concurrency_policy,
            enabled, next_run_at, metadata
        )
        VALUES (
            $1, $2, $3, $4, 'Daily inventory auto-update', '0 3 * * *',
            'UTC', 'fire_once', 'skip_if_running', false, NULL, $5
        )
        ON CONFLICT (id) DO UPDATE
        SET playbook_id = EXCLUDED.playbook_id,
            target_set_id = EXCLUDED.target_set_id,
            service_principal_id = EXCLUDED.service_principal_id,
            name = EXCLUDED.name,
            metadata = playbook_schedule.metadata || EXCLUDED.metadata,
            updated_at = NOW()
        "#,
    )
    .bind(schedule_id)
    .bind(playbook_id)
    .bind(target_set_id)
    .bind(service_principal_id)
    .bind(&schedule_metadata)
    .execute(&mut **tx)
    .await
    .map_err(AppError::DatabaseSource)?;

    Ok(())
}

/// Neutral dynamic selector of the built-in inventory target set. Must stay
/// domain-neutral: no customer role names, hostnames, or site literals.
fn system_inventory_target_selector() -> Value {
    json!({
        "runtime": ["ansible"],
        "target_kind": ["connection_target"],
        "connection_protocol": ["ssh"]
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Content sha of the shipped system inventory playbook. This is the
    /// version dedup key on every existing install: if the catalog file's
    /// bytes drift, each upgraded stage/GRAD instance mints a spurious
    /// version 2 on first boot.
    const SYSTEM_INVENTORY_CONTENT_SHA256: &str =
        "b849fbaa475ab5166c7f2765fa07d2afc03e9cef06865de41ca24347e8ce28dd";

    fn repo_catalog_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks")
    }

    #[test]
    fn repo_catalog_loads_and_system_content_sha_is_locked() {
        let catalog =
            load_official_catalog(&repo_catalog_dir()).expect("repo catalog must load cleanly");
        let system_entry = catalog
            .entries
            .iter()
            .find(|entry| entry.manifest.slug == SYSTEM_INVENTORY_PLAYBOOK_SLUG)
            .expect("repo catalog must ship the system inventory playbook");
        assert!(system_entry.manifest.system);
        assert!(system_entry.manifest.inventory_rest_enabled);
        assert_eq!(
            system_entry.content_sha256, SYSTEM_INVENTORY_CONTENT_SHA256,
            "system inventory playbook bytes drifted; existing installs would mint a spurious new version"
        );
        assert_eq!(system_entry.manifest.timeout_seconds, 90);
        assert!(system_entry.vars_schema.is_object());
        assert!(
            system_entry.vars_schema["properties"]["piping_inventory_write_platform"].is_object()
        );
    }

    #[test]
    fn repo_catalog_loads_non_system_low_risk_diagnostics_playbooks_with_strict_schemas() {
        let catalog =
            load_official_catalog(&repo_catalog_dir()).expect("repo catalog must load cleanly");
        let expected = [
            ("bounded-diagnostics-collection", "diagnostics"),
            ("x509-certificate-inspection", "security"),
            ("read-only-service-health", "health"),
        ];

        for (slug, category) in expected {
            let entry = catalog
                .entries
                .iter()
                .find(|entry| entry.manifest.slug == slug)
                .unwrap_or_else(|| panic!("repo catalog must ship `{slug}`"));
            assert!(!entry.manifest.system, "`{slug}` must remain non-system");
            assert!(!entry.manifest.inventory_rest_enabled);
            assert_eq!(entry.manifest.risk, "low");
            assert_eq!(entry.manifest.category, category);
            assert_eq!(entry.manifest.connection_default, "ssh");
            assert_eq!(entry.vars_schema["type"], "object");
            assert_eq!(entry.vars_schema["additionalProperties"], false);
            assert!(entry.vars_schema["properties"].is_object());
            assert!(!entry.content.is_empty());
        }
    }
    #[test]
    fn system_slug_resolves_to_fixed_historical_uuid() {
        let catalog = load_official_catalog(&repo_catalog_dir()).expect("repo catalog");
        let system_entry = catalog
            .entries
            .iter()
            .find(|entry| entry.manifest.system)
            .expect("system entry");
        let id = resolve_playbook_id(&system_entry.manifest).expect("id resolves");
        assert_eq!(id.to_string(), SYSTEM_INVENTORY_PLAYBOOK_ID);
        // The fixed id is intentionally NOT the uuidv5 derivation.
        let derived = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("{OFFICIAL_PLAYBOOK_UUID_PREFIX}{SYSTEM_INVENTORY_PLAYBOOK_SLUG}").as_bytes(),
        );
        assert_ne!(id, derived);
    }

    #[test]
    fn non_system_entry_derives_stable_uuidv5_from_slug() {
        let raw = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: official-disk-cleanup
    name: Disk cleanup
    description: Test entry
    category: maintenance
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: maintenance/disk-cleanup.yml
    timeout_seconds: 600
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
"#;
        let manifest = parse_manifest(raw).expect("manifest parses");
        let entry = &manifest.playbooks[0];
        let first = resolve_playbook_id(entry).expect("id resolves");
        let second = resolve_playbook_id(entry).expect("id resolves");
        assert_eq!(first, second, "derived id must be deterministic");
        assert_eq!(
            first,
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                b"piping-official-playbook:official-disk-cleanup"
            )
        );
    }

    #[test]
    fn explicit_manifest_id_wins_for_non_system_entries() {
        let raw = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: official-disk-cleanup
    id: 0198a1b2-0000-7000-8000-00000000abcd
    name: Disk cleanup
    description: Test entry
    category: maintenance
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: maintenance/disk-cleanup.yml
    timeout_seconds: 600
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
"#;
        let manifest = parse_manifest(raw).expect("manifest parses");
        let id = resolve_playbook_id(&manifest.playbooks[0]).expect("id resolves");
        assert_eq!(id.to_string(), "0198a1b2-0000-7000-8000-00000000abcd");
    }

    #[test]
    fn system_flag_on_unknown_slug_is_rejected() {
        let raw = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: rogue-system-playbook
    name: Rogue
    description: Test entry
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: rogue.yml
    timeout_seconds: 90
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
    system: true
"#;
        let manifest = parse_manifest(raw).expect("manifest parses");
        let error =
            resolve_playbook_id(&manifest.playbooks[0]).expect_err("unknown system slug rejected");
        assert!(error.to_string().contains("system"));
    }

    const OVERLAY_MANIFEST: &str = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: official-disk-cleanup
    name: Disk cleanup
    description: Test entry
    category: maintenance
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: maintenance/disk-cleanup.yml
    timeout_seconds: 600
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
"#;

    #[test]
    fn a_valid_overlay_wins_and_an_unusable_one_loses_without_failing_startup() {
        let scratch =
            std::env::temp_dir().join(format!("official-catalog-resolve-{}", Uuid::new_v4()));
        std::fs::create_dir_all(scratch.join("maintenance")).expect("scratch dir");

        let config = OfficialCatalogConfig {
            work_dir: scratch.to_string_lossy().to_string(),
            baked_dir: repo_catalog_dir().to_string_lossy().to_string(),
            ..OfficialCatalogConfig::default()
        };

        // No manifest at all in the overlay -> the baked catalog loads, and
        // silently, because an absent overlay is the normal state.
        let (dir, _, warnings) = load_configured_catalog(&config).expect("baked load");
        assert_ne!(dir, scratch, "an overlay with no manifest must not win");
        assert!(
            warnings.is_empty(),
            "an absent overlay is not a degradation"
        );

        // A manifest that does not parse -> still the baked catalog, reported.
        std::fs::write(scratch.join(OFFICIAL_CATALOG_MANIFEST_FILE), "not: [valid")
            .expect("write invalid manifest");
        let (dir, _, warnings) = load_configured_catalog(&config).expect("baked load");
        assert_ne!(dir, scratch, "an unparseable overlay must not win");
        assert!(!warnings.is_empty(), "an unusable overlay must be reported");

        // A manifest that parses, with its entrypoint present -> overlay wins.
        std::fs::write(
            scratch.join(OFFICIAL_CATALOG_MANIFEST_FILE),
            OVERLAY_MANIFEST,
        )
        .expect("write valid manifest");
        std::fs::write(
            scratch.join("maintenance/disk-cleanup.yml"),
            "- hosts: all\n  tasks: []\n",
        )
        .expect("write entrypoint");
        let (dir, _, warnings) = load_configured_catalog(&config).expect("overlay load");
        assert_eq!(dir, scratch, "a usable overlay must win");
        assert!(warnings.is_empty(), "a usable overlay is not a degradation");

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn a_missing_baked_catalog_says_so_instead_of_loading_the_dev_directory_silently() {
        // An empty baked dir is what a container gets when the image was built
        // without the catalog. The loader then reaches for a path relative to
        // the working directory, which under `cargo test` is the repo's own
        // catalog - so the load SUCCEEDS and the process carries on with a
        // directory nobody configured. That is the dangerous case: it is
        // indistinguishable from a correct start unless the loader says so.
        let empty_baked =
            std::env::temp_dir().join(format!("official-catalog-nobaked-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&empty_baked).expect("scratch dir");

        let config = OfficialCatalogConfig {
            work_dir: String::new(),
            baked_dir: empty_baked.to_string_lossy().to_string(),
            ..OfficialCatalogConfig::default()
        };
        let (dir, _catalog, warnings) =
            load_configured_catalog(&config).expect("the dev directory does load here");

        assert_ne!(dir, empty_baked, "the empty baked dir cannot have loaded");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains(LOCAL_DEV_FALLBACK_DIR)),
            "loading the local-dev catalog must be reported, got {warnings:?}"
        );

        std::fs::remove_dir_all(&empty_baked).ok();
    }

    #[test]
    fn broken_overlay_falls_back_to_baked_catalog_with_warning() {
        let scratch =
            std::env::temp_dir().join(format!("official-catalog-degraded-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");

        // A syntactically-valid manifest whose entrypoint file does NOT exist
        // (an interrupted refresh checkout): the overlay must lose to the
        // baked snapshot with a warning instead of failing startup.
        let manifest = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: official-disk-cleanup
    name: Disk cleanup
    description: Test entry
    category: maintenance
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: maintenance/disk-cleanup.yml
    timeout_seconds: 600
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
"#;
        std::fs::write(scratch.join(OFFICIAL_CATALOG_MANIFEST_FILE), manifest)
            .expect("write overlay manifest");

        let config = OfficialCatalogConfig {
            work_dir: scratch.to_string_lossy().to_string(),
            baked_dir: repo_catalog_dir().to_string_lossy().to_string(),
            ..OfficialCatalogConfig::default()
        };
        let (dir, catalog, warnings) =
            load_configured_catalog(&config).expect("fallback load succeeds");
        assert_ne!(dir, scratch, "broken overlay must not win");
        assert!(!warnings.is_empty(), "fallback must surface a warning");
        assert!(
            warnings
                .iter()
                .all(|warning| !warning.contains(LOCAL_DEV_FALLBACK_DIR)),
            "a present baked catalog must not reach the local-development one, got {warnings:?}"
        );
        assert!(
            catalog
                .entries
                .iter()
                .any(|entry| entry.manifest.slug == SYSTEM_INVENTORY_PLAYBOOK_SLUG),
            "baked catalog must load the system entry"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn catalog_file_symlink_is_rejected() {
        let scratch =
            std::env::temp_dir().join(format!("official-catalog-symlink-{}", Uuid::new_v4()));
        std::fs::create_dir_all(scratch.join("system")).expect("scratch dir");
        let secret = scratch.join("outside-secret.txt");
        std::fs::write(&secret, "leak").expect("write outside file");

        let manifest = r#"apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: https://github.com/mlimarenko/piping
  base_path: ansible/playbooks
playbooks:
  - slug: official-symlinked
    name: Symlinked entry
    description: Test entry
    category: maintenance
    risk: low
    runtime: ansible
    connection_default: ssh
    entrypoint_file: system/entry.yml
    timeout_seconds: 600
    output_schema: {}
    token_policy:
      ttl_seconds: 3600
      scopes: []
    launch_options: {}
    vars_schema: {}
"#;
        let catalog_dir = scratch.join("catalog");
        std::fs::create_dir_all(catalog_dir.join("system")).expect("catalog dir");
        std::fs::write(catalog_dir.join(OFFICIAL_CATALOG_MANIFEST_FILE), manifest)
            .expect("write manifest");
        std::os::unix::fs::symlink(&secret, catalog_dir.join("system/entry.yml"))
            .expect("plant symlink");

        let error = load_official_catalog(&catalog_dir).expect_err("symlink must be rejected");
        assert!(
            error.to_string().contains("regular file"),
            "unexpected error: {error}"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn version_dedup_predicate_metadata_matches_seeded_shape() {
        // The dedup SELECT matches on `metadata @> factory_metadata`. The
        // factory metadata built from the manifest must therefore stay a
        // subset of what existing version rows carry:
        // {"system": true, "factory": true, "launch_options": {...}}.
        let catalog = load_official_catalog(&repo_catalog_dir()).expect("repo catalog");
        let system_entry = catalog
            .entries
            .iter()
            .find(|entry| entry.manifest.system)
            .expect("system entry");
        let launch_options = &system_entry.manifest.launch_options;
        // Lock the exact key SET, not just values: `@>` is a subset check, so
        // ADDING a launch_options key to the manifest makes the factory
        // metadata a non-subset of every already-stored version row and mints
        // a spurious version on each existing install's next boot. Growing
        // this list is a deliberate act: it requires shipping a content change
        // (new version) anyway, or accepting that one-time version bump.
        let mut keys: Vec<&str> = launch_options
            .as_object()
            .expect("launch_options object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "ansible_timeout",
                "compact_events",
                "deprecation_warnings",
                "display_ok_hosts",
                "display_skipped_hosts",
                "execution_backend",
                "forks",
                "only_failed_event_data",
                "pipelining",
                "python_interpreter",
                "remote_tmp",
                "shell_executable",
                "ssh_common_args",
                "stdout_callback",
                "strategy",
                "timeout_seconds",
            ],
        );
        assert_eq!(launch_options["forks"], json!(64));
        assert_eq!(launch_options["timeout_seconds"], json!(90));
        assert_eq!(launch_options["execution_backend"], json!("native"));
        assert_eq!(launch_options["strategy"], json!("free"));
        assert_eq!(launch_options["python_interpreter"], json!("auto_silent"));
        assert_eq!(launch_options["ansible_timeout"], json!(12));
        assert_eq!(launch_options["display_ok_hosts"], json!(false));
        assert_eq!(launch_options["display_skipped_hosts"], json!(false));
        assert_eq!(launch_options["only_failed_event_data"], json!(true));
        assert_eq!(
            launch_options["ssh_common_args"],
            json!(
                "-o ConnectTimeout=8 -o ServerAliveInterval=8 -o ServerAliveCountMax=1 -o GSSAPIAuthentication=no"
            )
        );
    }

    #[test]
    fn target_selector_is_domain_neutral() {
        let selector = system_inventory_target_selector();
        assert_eq!(selector["runtime"], json!(["ansible"]));
        assert_eq!(selector["target_kind"], json!(["connection_target"]));
        assert_eq!(selector["connection_protocol"], json!(["ssh"]));
    }

    #[test]
    fn hex_sha256_matches_known_vector() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
