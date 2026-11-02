//! PWA-81 transfer application: pinned descriptor intake, target CAS,
//! authorization and apply/resume routing.
//!
//! Separation of concerns:
//!
//! * `bin::inventory_transfer` parses flags and prints a sanitized report;
//! * this module verifies every pinned input against live evidence, re-observes
//!   the target under a read-only snapshot, requires the operator's current
//!   token and permissions, and only then hands the prepared batch and the
//!   pinned descriptor to the canonical writer;
//! * `infrastructure::persistence::local_inventory_target` owns the database
//!   fence, the inventory mutation and the durable receipt.
//!
//! Nothing here bootstraps settings, runs migrations, seeds data, schedules work
//! or contacts a provider. Offline loading touches no database; target
//! observation runs only in `READ ONLY` transactions. The only writes of an
//! apply are the writer's inventory transaction and the canonical token-usage
//! touch.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::ApiTokenConfig;
use crate::contexts::access_control::application::{
    AccessControlService, PolicyEvaluationService, UserApiTokenService,
};
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    SqlxPolicyRepository, SqlxRoleRepository, SqlxUserApiTokenRepository, SqlxUserRepository,
};
use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestAuth, RequestContext};
use crate::contexts::sync::application::inventory_preflight::preflight;
use crate::contexts::sync::application::lock::ProviderLease;
use crate::contexts::sync::domain::inventory_export::{
    AuthorityKind, CaptureOutcome, ConsistencyStatus, SourceAttestationIdentity,
};
use crate::contexts::sync::domain::inventory_preflight::{Finding, PreflightReport};
use crate::contexts::sync::domain::{
    FieldKind, InventoryCommitAuthorisation, InventoryEntity, MappingConfigDocument,
    PinnedTransferDescriptor, RetainAbsencePolicy, TRANSFER_FORMAT_VERSION, TransferApproval,
    TransferDescriptor, TransferOperator, TransferReceipt, TransferRunState, TransferScope,
    TypedInventoryBatch, TypedInventoryRow,
};
use crate::contexts::sync::infrastructure::persistence::local_inventory_target::{
    LocalInventoryBatch, LocalInventoryCatalog, assess_preparation_from_source,
};
use crate::contexts::sync::infrastructure::providers::ImmutableInventorySource;
use crate::errors::AppError;

/// Bound on the provider lease. The lease is liveness only; PostgreSQL owns the
/// durable run and commit fence.
const LEASE_TTL_SECONDS: usize = 300;

type AccessControl =
    AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;
type PolicyEvaluation =
    PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;

/// A closed, sanitized failure. No database, HTTP, provider or operator text
/// crosses this boundary; `code` is a static identifier.
#[derive(Debug, Clone)]
pub enum TransferError {
    /// Verified inputs or the observed plan contain blockers; nothing written.
    Blocked {
        code: &'static str,
        findings: Vec<Finding>,
    },
    /// The target no longer matches the pinned identity, fingerprint or plan.
    Stale,
    /// Another writer holds the run fence or an observation lock.
    Busy,
    /// The operator token or its permissions do not allow this run.
    AuthorizationLost,
    /// A required boundary value cannot be obtained (executable, schema,
    /// database identity, receipt store, lease).
    Unavailable(&'static str),
    /// Another attempt owns the run.
    InProgress,
    /// The commit outcome cannot be observed; resolve it through the same run.
    Uncertain,
    /// The attempt failed before any durable write.
    RolledBack,
}

impl TransferError {
    #[must_use]
    pub fn state(&self) -> TransferRunState {
        match self {
            Self::Blocked { .. } => TransferRunState::Blocked,
            Self::Stale => TransferRunState::Stale,
            Self::Busy => TransferRunState::Busy,
            Self::AuthorizationLost => TransferRunState::AuthorizationLost,
            Self::Unavailable(_) => TransferRunState::RolledBack,
            Self::InProgress => TransferRunState::InProgress,
            Self::Uncertain => TransferRunState::Uncertain,
            Self::RolledBack => TransferRunState::RolledBack,
        }
    }

    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Blocked { code, .. } => code,
            Self::Stale => "inventory_transfer:stale_target",
            Self::Busy => "inventory_transfer:busy",
            Self::AuthorizationLost => "inventory_transfer:authorization_lost",
            Self::Unavailable(code) => code,
            Self::InProgress => "inventory_transfer:in_progress",
            Self::Uncertain => "inventory_transfer:uncertain_commit",
            Self::RolledBack => "inventory_transfer:rolled_back",
        }
    }

    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        match self {
            Self::Blocked { findings, .. } => findings,
            _ => &[],
        }
    }

    /// Exit code of the operator-facing report: 2 is a completed refusal, 1 is
    /// an environment or input failure that did not reach a decision.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Unavailable(_) => 1,
            _ => 2,
        }
    }
}

fn blocked(code: &'static str) -> TransferError {
    TransferError::Blocked {
        code,
        findings: Vec::new(),
    }
}

fn blocked_with(code: &'static str, findings: &[Finding]) -> TransferError {
    TransferError::Blocked {
        code,
        findings: findings.to_vec(),
    }
}

/// Reject a descriptor whose exact bytes do not match the independent pin.
/// A reserialized value is never used as the run identity.
pub fn pin_descriptor(
    descriptor_bytes: &[u8],
    descriptor_digest: &str,
) -> Result<PinnedTransferDescriptor, TransferError> {
    PinnedTransferDescriptor::parse(descriptor_bytes, descriptor_digest)
        .map_err(|_| blocked("inventory_transfer:descriptor_invalid"))
}

/// Boundary check of the pinned run approval. It is usable before any
/// connection exists, so an expired approval never reaches the target.
pub fn ensure_descriptor_current(
    pinned: &PinnedTransferDescriptor,
    now: DateTime<Utc>,
) -> Result<(), TransferError> {
    pinned
        .descriptor()
        .ensure_authority_current(now)
        .map_err(|_| blocked("inventory_transfer:approval_not_current"))
}

/// Sanitized operator-facing report of one attempt or refusal.
#[derive(Debug, Clone, Serialize)]
pub struct TransferReport {
    pub state: TransferRunState,
    pub code: &'static str,
    /// `None` only when the descriptor bytes could not be parsed at all.
    pub run_id: Option<Uuid>,
    pub descriptor_digest: Option<String>,
    pub receipt: Option<TransferReceipt>,
    pub findings: Vec<Finding>,
}

impl TransferReport {
    #[must_use]
    pub fn from_error(
        error: &TransferError,
        run_id: Option<Uuid>,
        descriptor_digest: Option<String>,
    ) -> Self {
        Self {
            state: error.state(),
            code: error.code(),
            run_id,
            descriptor_digest,
            receipt: None,
            findings: error.findings().to_vec(),
        }
    }
}

/// Verified offline inputs of one transfer. No database is touched.
pub struct OfflineTransfer {
    pub report: PreflightReport,
    /// Merkle identity of the sealed artifact, derived from its verified bytes.
    pub artifact_sha256: String,
    /// The sealed snapshot attestation, present only when the profile pinned it.
    pub authority: SourceAttestationIdentity,
    batch: LocalInventoryBatch,
}

/// Open and fully verify the sealed bundle, assess it and build the canonical
/// typed batch. A missing, altered, incomplete or unproven artifact blocks before
/// any database connection.
pub fn load_offline(
    bundle: &Path,
    bundle_digest: &str,
    provider_id: Uuid,
    mapping_bytes: &[u8],
    profile_bytes: &[u8],
    profile_digest: &str,
) -> Result<OfflineTransfer, TransferError> {
    let source = ImmutableInventorySource::open(bundle, bundle_digest, provider_id, 100)
        .map_err(|_| blocked("inventory_transfer:bundle_unverified"))?;
    if source.manifest().authority.kind == AuthorityKind::OperatorEndorsedSnapshot {
        crate::contexts::sync::infrastructure::providers::netbox_snapshot_capture::verify_snapshot_provenance(&source)
            .map_err(|_| blocked("inventory_transfer:source_provenance_mismatch"))?;
    }
    let capture = &source.manifest().capture;
    if capture.outcome != CaptureOutcome::Complete
        || capture.consistency != ConsistencyStatus::Consistent
        || !capture.discovery_complete
    {
        // An incomplete or unproven capture is never accepted, including a
        // operator-endorsed one.
        return Err(blocked("inventory_transfer:source_incomplete"));
    }
    let assessment = preflight(&source, mapping_bytes, profile_bytes, profile_digest)
        .map_err(|_| blocked("inventory_transfer:preflight_failed"))?;
    if !assessment.can_plan_target() {
        return Err(blocked_with(
            "inventory_transfer:preflight_blocked",
            &assessment.report.findings,
        ));
    }
    if !assessment.report.source_coverage_complete {
        return Err(blocked_with(
            "inventory_transfer:source_incomplete",
            &assessment.report.findings,
        ));
    }
    let report = assessment.report;
    let config: MappingConfigDocument = serde_json::from_slice(mapping_bytes)
        .map_err(|_| blocked("inventory_transfer:mapping_encoding"))?;
    if config.reference != report.mapping_ref
        || config.source_provider_id != report.source_provider_id
    {
        return Err(blocked("inventory_transfer:identity_mismatch"));
    }
    // Authority is evidence, not a claim: only a sealed snapshot attestation
    // that the independently pinned profile matched can authorize a run.
    if report.authority.kind != AuthorityKind::OperatorEndorsedSnapshot
        || !report.authority.attested
    {
        return Err(blocked("inventory_transfer:source_authority_unattested"));
    }
    let authority = report
        .authority
        .attestation
        .clone()
        .ok_or_else(|| blocked("inventory_transfer:source_authority_unattested"))?;
    // One validation owner for the typed batch: the same `assess_preparation`
    // the pull path's `prepare` delegates to. There is no second parser or
    // importer, and this step needs no connection.
    let prepared = assess_preparation_from_source(&config, &source);
    let batch = prepared.batch.ok_or_else(|| {
        blocked_with("inventory_transfer:preparation_blocked", &prepared.findings)
    })?;
    Ok(OfflineTransfer {
        report,
        // The source owner derives the artifact identity from every verified
        // original payload before it discards resource bytes.
        artifact_sha256: source.artifact_digest().to_owned(),
        authority,
        batch,
    })
}

/// Operator decisions of a descriptor preparation. Every other descriptor field
/// is derived from the artifact, the compiled sources or the observed target.
pub struct PreparationRequest {
    pub run_id: Uuid,
    pub operator_id: Uuid,
    pub operator_token: String,
    pub endorsed_provenance_sha256: String,
    /// The only operator-supplied authority input: when the run may still run.
    pub approval: TransferApproval,
    pub scope: TransferScope,
}

/// A descriptor bound to its exact bytes plus the assessment it was built from.
pub struct PreparedTransfer {
    pub pinned: PinnedTransferDescriptor,
    pub report: PreflightReport,
}

/// Prepare an immutable descriptor from verified offline inputs and the
/// canonical read-only target observation.
pub async fn prepare_descriptor(
    pool: &PgPool,
    request: PreparationRequest,
    offline: OfflineTransfer,
) -> Result<PreparedTransfer, TransferError> {
    let context = authenticate_operator(pool, &request.operator_token, Utc::now())
        .await
        .map_err(|_| TransferError::AuthorizationLost)?;
    if context.user_id != Some(request.operator_id)
        || request.operator_id != offline.authority.operator_id
        || request.endorsed_provenance_sha256 != offline.authority.provenance_sha256
    {
        return Err(TransferError::AuthorizationLost);
    }
    request
        .approval
        .ensure_current(Utc::now())
        .map_err(|_| blocked("inventory_transfer:approval_not_current"))?;
    // The supported importer rejects secret custom-field values upstream and has
    // no secret-intent writer, so a declared secret scope is refused instead of
    // inventing a grant or a write path for it.
    if request.scope.secret_intents {
        return Err(blocked("inventory_transfer:secret_scope_unsupported"));
    }
    let scope = required_scope(&request.scope, &offline.batch);
    ensure_scope_permissions(pool, &context, &scope)
        .await
        .map_err(|_| TransferError::AuthorizationLost)?;
    let target = LocalInventoryCatalog::new(pool.clone());
    let (observed_target, schema_digest) = target
        .transfer_observation(&offline.batch)
        .await
        .map_err(|error| observation_error(&error))?;
    // The descriptor pins what the batch actually requires, so a later reader
    // cannot inherit an under-stated claim.
    let report = offline.report;
    let descriptor = TransferDescriptor {
        format_version: TRANSFER_FORMAT_VERSION,
        run_id: request.run_id,
        provider_id: report.source_provider_id,
        bundle_digest: report.bundle_digest.clone(),
        mapping_ref: report.mapping_ref,
        mapping_digest: report.mapping_digest.clone(),
        profile_digest: report.profile_digest.clone(),
        catalog_digest: report.catalog_digest.clone(),
        tool_digest: report.tool_digest.clone(),
        schema_digest,
        artifact_sha256: offline.artifact_sha256,
        executable_sha256: executable_digest()?,
        target: observed_target,
        operator: TransferOperator {
            user_id: request.operator_id,
        },
        authority: offline.authority,
        approval: request.approval,
        retain_absence: RetainAbsencePolicy::RetainAndReport,
        scope,
    };
    let pinned = descriptor
        .pin()
        .map_err(|_| blocked("inventory_transfer:descriptor_invalid"))?;
    Ok(PreparedTransfer { pinned, report })
}

/// Inputs of one apply/resume attempt.
pub struct ApplyRequest<'a> {
    /// Exact descriptor bytes; never re-encoded for identity.
    pub descriptor_bytes: &'a [u8],
    pub descriptor_digest: &'a str,
    pub bundle: &'a Path,
    pub mapping_bytes: &'a [u8],
    pub profile_bytes: &'a [u8],
    /// Raw operator bearer token from the process environment.
    pub operator_token: &'a str,
    pub endorsed_provenance_sha256: &'a str,
    /// `true` for `resume`, `false` for `apply`. Both share one idempotent path:
    /// the run identity is the descriptor digest.
    pub resume: bool,
}

pub struct TransferApplication {
    pool: PgPool,
    redis: ConnectionManager,
    target: LocalInventoryCatalog,
    lease_ttl_seconds: usize,
}

impl TransferApplication {
    #[must_use]
    pub fn new(pool: PgPool, redis: ConnectionManager) -> Self {
        Self {
            target: LocalInventoryCatalog::new(pool.clone()),
            pool,
            redis,
            lease_ttl_seconds: LEASE_TTL_SECONDS,
        }
    }

    /// Apply or resume one pinned run.
    ///
    /// Order is deliberate: descriptor intake and authorization first, then the
    /// durable receipt of this exact run, then every live-evidence re-check. A
    /// completed run therefore returns its receipt even if the target changed
    /// after the commit, and later edits are never rewritten.
    pub async fn apply(&self, request: ApplyRequest<'_>) -> Result<TransferReport, TransferError> {
        let now = Utc::now();
        let pinned = pin_descriptor(request.descriptor_bytes, request.descriptor_digest)?;
        let descriptor = pinned.descriptor();

        // Authorization is required even for a receipt replay. The declared
        // scope is a claim; the prepared batch decides the effective one below.
        let context = authenticate_operator(&self.pool, request.operator_token, now)
            .await
            .map_err(|_| TransferError::AuthorizationLost)?;
        if context.user_id != Some(descriptor.operator.user_id)
            || descriptor.operator.user_id != descriptor.authority.operator_id
            || request.endorsed_provenance_sha256 != descriptor.authority.provenance_sha256
        {
            return Err(TransferError::AuthorizationLost);
        }
        ensure_scope_permissions(&self.pool, &context, &descriptor.scope)
            .await
            .map_err(|_| TransferError::AuthorizationLost)?;

        let replay = self
            .target
            .transfer_receipt(descriptor.run_id, pinned.descriptor_digest())
            .await
            .map_err(|_| TransferError::Unavailable("inventory_transfer:receipt_unavailable"))?;
        if let Some(receipt) = replay {
            receipt
                .verify(descriptor.run_id, pinned.descriptor_digest())
                .map_err(|_| blocked("inventory_transfer:receipt_mismatch"))?;
            return Ok(TransferReport {
                state: TransferRunState::Replayed,
                code: "inventory_transfer:replayed",
                run_id: Some(descriptor.run_id),
                descriptor_digest: Some(pinned.descriptor_digest().to_owned()),
                receipt: Some(receipt),
                findings: Vec::new(),
            });
        }
        let committed_code = if request.resume {
            "inventory_transfer:resumed"
        } else {
            "inventory_transfer:committed"
        };
        // The authority window gates writes, not evidence retrieval: a committed
        // run returns its receipt above even after the window closed. Every
        // mutation path still requires a current window here and again inside
        // the writer's fence.
        ensure_descriptor_current(&pinned, now)?;
        if descriptor.scope.secret_intents {
            return Err(blocked("inventory_transfer:secret_scope_unsupported"));
        }

        if executable_digest()? != descriptor.executable_sha256 {
            return Err(blocked("inventory_transfer:executable_mismatch"));
        }
        let offline = load_offline(
            request.bundle,
            &descriptor.bundle_digest,
            descriptor.provider_id,
            request.mapping_bytes,
            request.profile_bytes,
            &descriptor.profile_digest,
        )?;
        if offline.artifact_sha256 != descriptor.artifact_sha256 {
            return Err(blocked("inventory_transfer:artifact_mismatch"));
        }
        if offline.authority != descriptor.authority {
            // The sealed attestation is not the one this descriptor pinned.
            return Err(blocked("inventory_transfer:authority_mismatch"));
        }
        ensure_pinned_identities(&offline.report, descriptor)?;
        // Derive the requirements from the prepared rows, not from the caller's
        // booleans: a declared scope that under-states the batch cannot omit a
        // permission check.
        let required = required_scope(&descriptor.scope, &offline.batch);
        if required != descriptor.scope {
            // The prepared effects differ from the pinned requirements: a
            // different run, never a broader one under the same descriptor.
            return Err(blocked("inventory_transfer:scope_mismatch"));
        }
        ensure_scope_permissions(&self.pool, &context, &required)
            .await
            .map_err(|_| TransferError::AuthorizationLost)?;
        let (observed_target, schema_digest) = self
            .target
            .transfer_observation(&offline.batch)
            .await
            .map_err(|error| observation_error(&error))?;
        if schema_digest != descriptor.schema_digest {
            return Err(blocked("inventory_transfer:schema_mismatch"));
        }
        if observed_target != descriptor.target {
            // The target, fingerprint or resolved plan moved after the
            // descriptor was pinned. Never silently re-plan.
            return Err(TransferError::Stale);
        }

        let lease = ProviderLease::acquire(
            self.redis.clone(),
            descriptor.provider_id,
            self.lease_ttl_seconds,
        )
        .await
        .map_err(|error| match error {
            AppError::Conflict(_) => TransferError::Busy,
            _ => TransferError::Unavailable("inventory_transfer:lease_unavailable"),
        })?;

        let target = self.target.clone();
        let run_id = descriptor.run_id;
        let descriptor_digest = pinned.descriptor_digest().to_owned();
        let batch = offline.batch;
        let pool = self.pool.clone();
        let operator_token = request.operator_token.to_owned();
        let operator_user_id = descriptor.operator.user_id;
        let attempt = lease
            .run(|lease_authorisation| async move {
                // The writer re-checks ownership before commit; that check must
                // also re-validate the operator token and permissions, so the
                // lease is wrapped instead of handed over directly.
                let authorisation: Arc<dyn InventoryCommitAuthorisation> =
                    Arc::new(OperatorCommitAuthorisation {
                        lease: lease_authorisation,
                        pool,
                        operator_token,
                        operator_user_id,
                        required,
                    });
                target.apply_transfer(batch, &pinned, authorisation).await
            })
            .await;
        match attempt {
            Ok(receipt) => {
                receipt
                    .verify(run_id, &descriptor_digest)
                    .map_err(|_| TransferError::Uncertain)?;
                Ok(TransferReport {
                    state: TransferRunState::Committed,
                    code: committed_code,
                    run_id: Some(run_id),
                    descriptor_digest: Some(descriptor_digest),
                    receipt: Some(receipt),
                    findings: Vec::new(),
                })
            }
            Err(error) => {
                let classification = classify_writer_error(&error);
                // `ProviderLease::run` may drop the work future while the commit
                // is in flight (for example when lease renewal fails). The
                // durable run fence, not this process, decides the outcome:
                // recover the committed receipt when it exists and never claim a
                // rollback that was not observed.
                if matches!(
                    &classification,
                    TransferError::Uncertain
                        | TransferError::AuthorizationLost
                        | TransferError::RolledBack
                ) && let Ok(Some(receipt)) = self
                    .target
                    .transfer_receipt(run_id, &descriptor_digest)
                    .await
                    && receipt.verify(run_id, &descriptor_digest).is_ok()
                {
                    return Ok(TransferReport {
                        state: TransferRunState::Committed,
                        code: "inventory_transfer:committed_recovered",
                        run_id: Some(run_id),
                        descriptor_digest: Some(descriptor_digest),
                        receipt: Some(receipt),
                        findings: Vec::new(),
                    });
                }
                Err(classification)
            }
        }
    }
}

/// Validate the operator's bearer token: exact hash, active status, expiry,
/// revocation and an active user. A business owner is never a principal.
async fn authenticate_operator(
    pool: &PgPool,
    operator_token: &str,
    now: DateTime<Utc>,
) -> Result<RequestContext, AppError> {
    if operator_token.trim().is_empty() {
        return Err(AppError::Auth(
            "inventory_transfer:authorization_lost".into(),
        ));
    }
    let service = UserApiTokenService::new(
        Arc::new(SqlxUserApiTokenRepository::new(pool.clone())),
        Arc::new(SqlxUserRepository::new(pool.clone())),
        ApiTokenConfig::default(),
        None,
    );
    let validated = service.validate_bearer_token(operator_token, now).await?;
    let mut context = RequestContext::new(
        Some(validated.user_id),
        "api_token".to_string(),
        None,
        Uuid::now_v7().to_string(),
        RequestAuth {
            api_token_scopes: validated
                .scopes
                .iter()
                .map(ApiTokenScopeContext::from)
                .collect(),
            ..RequestAuth::default()
        },
    );
    context.api_token_id = Some(validated.token_id.to_string());
    Ok(context)
}

/// Enforce the canonical permissions required by a scope.
///
/// Business owners and owner groups are inventory resources: they are
/// created/updated under the same collection grant as any other inventory row,
/// and are never resolved as access-control users. Populated
/// `users.users`/`users.groups` memberships stay blocked upstream because no
/// auth-identity conversion exists; this mapping never invents one.
async fn ensure_scope_permissions(
    pool: &PgPool,
    context: &RequestContext,
    scope: &TransferScope,
) -> Result<(), AppError> {
    let access = AccessControl::new(
        SqlxUserRepository::new(pool.clone()),
        SqlxRoleRepository::new(pool.clone()),
        SqlxPolicyRepository::new(pool.clone()),
    );
    let policies = PolicyEvaluation::new(access);
    // Assigning owners may create a missing owner or owner group.
    if (scope.creates || scope.owner_assignments)
        && policies
            .ensure_inventory_collection_access(context, Action::Create)
            .await
            .is_err()
    {
        return Err(AppError::Forbidden(
            "inventory_transfer:authorization_lost".into(),
        ));
    }
    // Reference resolution requires both collection access here and the
    // persisted target's per-object access at the writer boundary.
    if scope_requires_inventory_read(scope)
        && policies
            .ensure_inventory_collection_access(context, Action::Read)
            .await
            .is_err()
    {
        return Err(AppError::Forbidden(
            "inventory_transfer:authorization_lost".into(),
        ));
    }
    if policies
        .ensure_inventory_collection_access(context, Action::Update)
        .await
        .is_err()
    {
        return Err(AppError::Forbidden(
            "inventory_transfer:authorization_lost".into(),
        ));
    }
    if scope.secret_intents
        && policies
            .ensure_access(context, Action::Secret, ResourceType::SecretVariable, None)
            .await
            .is_err()
    {
        return Err(AppError::Forbidden(
            "inventory_transfer:authorization_lost".into(),
        ));
    }
    Ok(())
}

/// Whether the run must read inventory rows it resolves.
///
/// The effective scope marks `object_references` for every reference-shaped
/// effect, including custom-field JSON intent, so this single predicate is the
/// gate used by the permission check.
fn scope_requires_inventory_read(scope: &TransferScope) -> bool {
    scope.object_references || scope.owner_assignments
}

/// Union of the declared scope and the effects actually prepared by the
/// assessment. False booleans in the descriptor can add requirements but never
/// remove one.
fn required_scope(declared: &TransferScope, batch: &LocalInventoryBatch) -> TransferScope {
    let effective = effective_scope(batch);
    TransferScope {
        creates: declared.creates || effective.creates,
        updates: declared.updates || effective.updates,
        owner_assignments: declared.owner_assignments || effective.owner_assignments,
        custom_fields: declared.custom_fields || effective.custom_fields,
        object_references: declared.object_references || effective.object_references,
        secret_intents: declared.secret_intents || effective.secret_intents,
    }
}

/// Requirements observable in the prepared batch.
///
/// The batch is the canonical statement of what will be written: the writer's
/// row owner exposes the actual mapped intent (`has_field`: scalar, reference,
/// junction, tag or explicit clear; `has_custom_fields`: a supplied custom-field
/// object). Nothing here is inferred from the entity's physical columns.
fn effective_scope(batch: &LocalInventoryBatch) -> TransferScope {
    let mut scope = TransferScope {
        // A prepared row may be inserted or updated; the planner, not the
        // caller, decides which, so both are required.
        creates: !batch.rows().is_empty(),
        updates: !batch.rows().is_empty(),
        owner_assignments: false,
        custom_fields: false,
        object_references: false,
        // No supported write path carries a secret intent.
        secret_intents: false,
    };
    for row in batch.rows() {
        let entity = row.entity();
        if is_owner_entity_key(entity.source_key()) {
            scope.owner_assignments = true;
        }
        // Any actual custom-field JSON intent — a supplied value object or a
        // custom-field definition/choice row — can carry Object/Multiobject
        // values whose definitions and targets are inventory rows. Resolving
        // them is an inventory read, so the requirement is not limited to
        // ordinary FK/junction/owner/tag intent.
        if entity.source_key().starts_with("extras.custom-field") || row.has_custom_fields() {
            scope.custom_fields = true;
            scope.object_references = true;
        }
        for field in entity.field_iter() {
            if !row.has_field(field) {
                continue;
            }
            match field.kind() {
                FieldKind::ForeignKey { target } => {
                    if target.is_some_and(is_owner_entity) {
                        scope.owner_assignments = true;
                    }
                    scope.object_references = true;
                }
                FieldKind::Junction { target, .. } => {
                    if target.is_some_and(is_owner_entity) {
                        scope.owner_assignments = true;
                    }
                    scope.object_references = true;
                }
                // Generic and content-type references resolve canonical rows, and
                // tag membership resolves tag rows.
                FieldKind::GenericReference { .. }
                | FieldKind::ContentTypeJunction { .. }
                | FieldKind::Tags => scope.object_references = true,
                FieldKind::Scalar => {}
            }
        }
    }
    scope
}

fn is_owner_entity(entity: InventoryEntity) -> bool {
    is_owner_entity_key(entity.source_key())
}

fn is_owner_entity_key(source_key: &str) -> bool {
    source_key == "users.owners" || source_key == "users.owner-groups"
}

/// Composite commit authorisation: the provider lease plus a fresh operator
/// token and permission check on every `ensure_owned` call, including the
/// writer's final boundary. A token that expires or is revoked mid-run cannot
/// commit.
struct OperatorCommitAuthorisation {
    lease: Arc<dyn InventoryCommitAuthorisation>,
    pool: PgPool,
    operator_token: String,
    operator_user_id: Uuid,
    required: TransferScope,
}

impl OperatorCommitAuthorisation {
    async fn fresh_context(&self) -> Result<RequestContext, AppError> {
        let context = authenticate_operator(&self.pool, &self.operator_token, Utc::now()).await?;
        if context.user_id != Some(self.operator_user_id) {
            return Err(AppError::Forbidden(
                "inventory_transfer:authorization_lost".into(),
            ));
        }
        ensure_scope_permissions(&self.pool, &context, &self.required).await?;
        Ok(context)
    }

    async fn ensure_object_access(
        &self,
        action: Action,
        id: Uuid,
        attributes: serde_json::Value,
    ) -> Result<(), AppError> {
        let serde_json::Value::Object(attributes) = attributes else {
            return Err(AppError::Forbidden(
                "inventory_transfer:authorization_lost".into(),
            ));
        };
        let context = self.fresh_context().await?;
        let access = AccessControl::new(
            SqlxUserRepository::new(self.pool.clone()),
            SqlxRoleRepository::new(self.pool.clone()),
            SqlxPolicyRepository::new(self.pool.clone()),
        );
        PolicyEvaluation::new(access)
            .ensure_access_with_attributes(
                &context,
                action,
                ResourceType::Inventory,
                Some(&format!("inventory:{id}")),
                Some(attributes.into_iter().collect()),
            )
            .await
    }
}

impl InventoryCommitAuthorisation for OperatorCommitAuthorisation {
    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            self.lease.ensure_owned().await?;
            self.fresh_context().await?;
            Ok(())
        })
    }

    fn ensure_reference_access(
        &self,
        id: Uuid,
        attributes: serde_json::Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(self.ensure_object_access(Action::Read, id, attributes))
    }

    fn ensure_mutation_access(
        &self,
        action: Action,
        id: Uuid,
        attributes: serde_json::Value,
    ) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            if action != Action::Create && action != Action::Update {
                return Err(AppError::Forbidden(
                    "inventory_transfer:authorization_lost".into(),
                ));
            }
            self.ensure_object_access(action, id, attributes).await
        })
    }
}

/// Map a failed canonical target observation onto the closed report vocabulary.
/// The local inventory target owns the observation; this only classifies it.
fn observation_error(error: &AppError) -> TransferError {
    match error {
        AppError::Conflict(message)
            if message == "inventory_transfer:stale_target"
                || message == "inventory_transfer:stale_plan"
                || message == "inventory_transfer:bound_row_missing" =>
        {
            TransferError::Stale
        }
        AppError::Conflict(message) if message == "inventory_transfer:schema_incomplete" => {
            blocked("inventory_transfer:schema_incomplete")
        }
        AppError::Validation(message) if message.contains("target_graph_blocked") => {
            blocked("inventory_transfer:target_plan_blocked")
        }
        _ => TransferError::Unavailable("inventory_transfer:target_observation_failed"),
    }
}

/// Compare every assessment identity with the pinned descriptor. A difference is
/// a different input set, never a re-plan.
fn ensure_pinned_identities(
    report: &PreflightReport,
    descriptor: &TransferDescriptor,
) -> Result<(), TransferError> {
    if report.bundle_digest != descriptor.bundle_digest
        || report.source_provider_id != descriptor.provider_id
        || report.mapping_ref != descriptor.mapping_ref
        || report.mapping_digest != descriptor.mapping_digest
        || report.profile_digest != descriptor.profile_digest
        || report.catalog_digest != descriptor.catalog_digest
        || report.tool_digest != descriptor.tool_digest
    {
        return Err(blocked("inventory_transfer:identity_mismatch"));
    }
    Ok(())
}

/// Digest of the running executable. Descriptor preparation and application must
/// run the same artifact; an unreadable executable is a boundary failure, not a
/// reason to trust a supplied string.
fn executable_digest() -> Result<String, TransferError> {
    use std::io::Read as _;

    let path = std::env::current_exe()
        .map_err(|_| TransferError::Unavailable("inventory_transfer:executable_unreadable"))?;
    let mut file = std::fs::File::open(path)
        .map_err(|_| TransferError::Unavailable("inventory_transfer:executable_unreadable"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| TransferError::Unavailable("inventory_transfer:executable_unreadable"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Map a writer failure onto the closed report vocabulary without exposing its
/// message.
///
/// The default is `Uncertain`, not `RolledBack`: a failed attempt whose work
/// future was dropped cannot prove that the transaction did not commit, and a
/// false rollback claim would invite a duplicate write. Only a validation
/// refusal, which is decided before any row is written, is reported as rolled
/// back.
fn classify_writer_error(error: &AppError) -> TransferError {
    match error {
        AppError::Conflict(message) if message == "inventory_transfer:busy" => TransferError::Busy,
        AppError::Conflict(message) if message == "inventory_transfer:in_progress" => {
            TransferError::InProgress
        }
        AppError::Conflict(message)
            if message == "inventory_transfer:descriptor_mismatch"
                || message == "inventory_transfer:run_missing" =>
        {
            TransferError::Stale
        }
        AppError::Conflict(message)
            if message == "inventory_transfer:stale_target"
                || message == "inventory_transfer:stale_plan"
                || message == "inventory_transfer:bound_row_missing" =>
        {
            TransferError::Stale
        }
        AppError::Conflict(message) if message == "inventory_transfer:schema_incomplete" => {
            blocked("inventory_transfer:schema_incomplete")
        }
        AppError::Conflict(message) if message == "inventory_transfer:provider_mismatch" => {
            blocked("inventory_transfer:identity_mismatch")
        }
        // `InventoryCommitAuthorisation::ensure_owned` reports a lost lease.
        AppError::Conflict(message) if message == "Sync lease ownership was lost" => {
            TransferError::AuthorizationLost
        }
        AppError::Validation(_)
        | AppError::ValidationFields(_)
        | AppError::ValidationJson(_)
        | AppError::ValidationUuid(_) => TransferError::RolledBack,
        AppError::Auth(_) | AppError::Forbidden(_) => TransferError::AuthorizationLost,
        _ => TransferError::Uncertain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::domain::inventory_sync::{
        SourceIdentity, SourceRecord, SourceSnapshot,
    };
    use crate::contexts::sync::domain::{
        Absent, ConflictMode, EntityMapping, FieldMapping, InventoryEntity,
        MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget, SourceEntity,
    };

    struct TestLease;

    impl InventoryCommitAuthorisation for TestLease {
        fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn ensure_reference_access(
            &self,
            _id: Uuid,
            _attributes: serde_json::Value,
        ) -> BoxFuture<'_, Result<(), AppError>> {
            // Lease ownership is not reference authorization.
            Box::pin(async { Err(AppError::Forbidden("lease is not a policy".into())) })
        }

        fn ensure_mutation_access(
            &self,
            _action: Action,
            _id: Uuid,
            _attributes: serde_json::Value,
        ) -> BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async { Err(AppError::Forbidden("lease is not a policy".into())) })
        }
    }

    #[tokio::test]
    #[ignore = "requires a migrated, disposable pwa81_transfer_ database"]
    async fn reference_access_checks_persisted_attributes_and_fresh_token()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::contexts::access_control::application::CreateUserApiTokenInput;
        use crate::contexts::access_control::domain::UserApiTokenScope;

        let url = std::env::var("PWA81_TRANSFER_TEST_DATABASE_URL")?;
        let pool = PgPool::connect(&url).await?;
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await?;
        assert!(
            database
                .strip_prefix("pwa81_transfer_")
                .is_some_and(|suffix| {
                    suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                }),
            "disposable database required"
        );

        let user = Uuid::now_v7();
        let template = Uuid::now_v7();
        let other_template = Uuid::now_v7();
        let manufacturer = Uuid::now_v7();
        let device_type = Uuid::now_v7();
        let name = format!("pwa81-{user}");
        sqlx::query("INSERT INTO identity_user (id, username, email, status, auth_source) VALUES ($1,$2,$3,'active','local')")
            .bind(user).bind(&name).bind(format!("{name}@example.test")).execute(&pool).await?;
        sqlx::query("INSERT INTO license_seat_assignment (user_id) VALUES ($1)")
            .bind(user)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO dcim_manufacturer(id,name,slug,description,comments,custom_field_data) VALUES ($1,$2,$2,'','','{}')")
            .bind(manufacturer).bind(&name).execute(&pool).await?;
        sqlx::query("INSERT INTO dcim_devicetype(id,model,slug,part_number,u_height,is_full_depth,front_image,rear_image,comments,manufacturer_id,description,console_port_template_count,console_server_port_template_count,power_port_template_count,power_outlet_template_count,interface_template_count,front_port_template_count,rear_port_template_count,device_bay_template_count,module_bay_template_count,inventory_item_template_count,exclude_from_utilization,device_count,custom_field_data) VALUES ($1,$2,$2,'',1,true,'','','',$3,'',0,0,0,0,0,0,0,0,0,0,false,0,'{}')")
            .bind(device_type).bind(&name).bind(manufacturer).execute(&pool).await?;
        for (id, template_name) in [
            (template, name.clone()),
            (other_template, format!("{name}-other")),
        ] {
            sqlx::query("INSERT INTO dcim_consoleporttemplate(id,name,label,description,device_type_id) VALUES ($1,$2,'','',$3)")
                .bind(id).bind(template_name).bind(device_type).execute(&pool).await?;
        }
        let subjects = serde_json::json!([format!("user:{user}")]);
        sqlx::query("INSERT INTO rbac_policy(id,name,effect,actions,resource_types,subjects,resources,enabled) VALUES ($1,$2,'allow',$3,$4,$5,$6,true)")
            .bind(Uuid::now_v7()).bind(format!("{name}-allow"))
            .bind(serde_json::json!([Action::Read, Action::Update]))
            .bind(serde_json::json!([ResourceType::Inventory]))
            .bind(&subjects).bind(serde_json::json!(["inventory:*"]))
            .execute(&pool).await?;
        sqlx::query("INSERT INTO rbac_policy(id,name,effect,actions,resource_types,constraints,subjects,resources,enabled) VALUES ($1,$2,'deny',$3,$4,$5,$6,$7,true)")
            .bind(Uuid::now_v7()).bind(format!("{name}-deny"))
            .bind(serde_json::json!([Action::Read]))
            .bind(serde_json::json!([ResourceType::Inventory]))
            .bind(serde_json::json!({"attributes":{"object_type":"dcim.consoleporttemplate", "device_type_id":device_type}}))
            .bind(&subjects).bind(serde_json::json!([format!("inventory:{template}")]))
            .execute(&pool).await?;
        let service = UserApiTokenService::new(
            Arc::new(SqlxUserApiTokenRepository::new(pool.clone())),
            Arc::new(SqlxUserRepository::new(pool.clone())),
            ApiTokenConfig::default(),
            None,
        );
        let create = || CreateUserApiTokenInput {
            name: "reference-test".into(),
            scopes: [Action::Read, Action::Update]
                .into_iter()
                .map(|action| UserApiTokenScope {
                    action,
                    resource_type: ResourceType::Inventory,
                    resource_pattern: "*".into(),
                })
                .collect(),
            expires_in_seconds: None,
        };
        let token = service.create_for_user(user, create(), Utc::now()).await?;
        let auth = OperatorCommitAuthorisation {
            lease: Arc::new(TestLease),
            pool: pool.clone(),
            operator_token: token.raw_token,
            operator_user_id: user,
            required: TransferScope {
                creates: false,
                updates: false,
                owner_assignments: false,
                custom_fields: false,
                object_references: true,
                secret_intents: false,
            },
        };
        let attributes: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(s) || jsonb_build_object('object_type', 'dcim.consoleporttemplate') FROM dcim_consoleporttemplate s WHERE id=$1")
                .bind(template)
                .fetch_one(&pool)
                .await?;
        let other_attributes: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(s) || jsonb_build_object('object_type', 'dcim.consoleporttemplate') FROM dcim_consoleporttemplate s WHERE id=$1")
                .bind(other_template)
                .fetch_one(&pool)
                .await?;
        assert!(auth.ensure_owned().await.is_ok());
        assert!(
            auth.ensure_reference_access(template, attributes.clone())
                .await
                .is_err()
        );
        assert!(
            auth.ensure_reference_access(other_template, other_attributes.clone())
                .await
                .is_ok()
        );
        assert!(
            auth.ensure_reference_access(template, serde_json::Value::Null)
                .await
                .is_err()
        );
        service
            .revoke_for_user(user, token.token.id, Utc::now())
            .await?;
        assert!(
            auth.ensure_reference_access(other_template, other_attributes.clone())
                .await
                .is_err()
        );

        let expiring = service.create_for_user(user, create(), Utc::now()).await?;
        let auth = OperatorCommitAuthorisation {
            operator_token: expiring.raw_token,
            ..auth
        };
        assert!(
            auth.ensure_reference_access(other_template, other_attributes.clone())
                .await
                .is_ok()
        );
        sqlx::query("UPDATE auth_user_api_token SET expires_at = $1 WHERE id = $2")
            .bind(Utc::now() - chrono::TimeDelta::seconds(1))
            .bind(expiring.token.id)
            .execute(&pool)
            .await?;
        assert!(
            auth.ensure_reference_access(other_template, other_attributes)
                .await
                .is_err()
        );
        pool.close().await;
        Ok(())
    }

    fn site_mapping(provider: Uuid) -> MappingConfigDocument {
        let reference = MappingRef::new(Uuid::now_v7(), 1);
        let mut config = MappingConfigDocument {
            reference,
            name: "authorization scope fixture".into(),
            description: None,
            source_provider_id: provider,
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Pull,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: Utc::now(),
            retired_at: None,
            entity_mappings: Vec::new(),
            field_mappings: Vec::new(),
        };
        let id = Uuid::now_v7();
        config.entity_mappings.push(EntityMapping {
            id,
            mapping_config_logical_id: reference.logical_id,
            mapping_config_version_id: reference.version_id,
            source_entity: InventoryEntity::Site.source_key().into(),
            target_entity: InventoryEntity::Site.target_key(),
            enabled: true,
            created_at: Utc::now(),
        });
        for field in InventoryEntity::Site.field_iter() {
            let mapped = matches!(field.name(), "name" | "slug");
            if !mapped && field.absent() != Absent::Reject {
                continue;
            }
            let expression = if mapped {
                serde_json::json!({"op": "identity", "args": [{"field": field.name()}]})
            } else {
                serde_json::json!({"op": "literal", "args": [if field.name() == "status" { "active" } else { "" }]})
            };
            config.field_mappings.push(FieldMapping {
                id: Uuid::now_v7(),
                entity_mapping_id: id,
                source_field: field.name().into(),
                target_field: field.name().into(),
                forward_transform_json: expression,
                reverse_transform_json: None,
                round_trip_critical: false,
                created_at: Utc::now(),
            });
        }
        config
    }

    fn batch(provider: Uuid, fields: serde_json::Value) -> LocalInventoryBatch {
        let config = site_mapping(provider);
        let record = SourceRecord::new(
            SourceIdentity::new(
                provider,
                SourceEntity::new("dcim.sites").expect("canonical entity"),
                "1",
            )
            .expect("identity"),
            fields.as_object().cloned().expect("object fields"),
        );
        crate::contexts::sync::infrastructure::persistence::local_inventory_target::assess_preparation(
            &config,
            &SourceSnapshot {
                records: vec![record],
            },
        )
        .batch
        .expect("synthetic preparation")
    }

    #[test]
    fn custom_field_intent_requires_an_inventory_read_and_never_a_secret_grant() {
        let custom = batch(
            Uuid::from_u128(1),
            serde_json::json!({
                "name": "synthetic-site",
                "slug": "synthetic-site",
                "custom_fields": {"note": "known"},
            }),
        );
        let scope = effective_scope(&custom);
        assert!(scope.custom_fields, "a supplied CF object is a CF write");
        assert!(
            scope.object_references,
            "CF JSON intent resolves definitions and targets, so Read is required"
        );
        assert!(
            !scope.secret_intents,
            "no supported write path carries a secret intent"
        );
        assert!(
            scope_requires_inventory_read(&scope),
            "the permission gate must require inventory Read for CF intent"
        );
        let declared = TransferScope {
            creates: true,
            updates: true,
            owner_assignments: false,
            custom_fields: false,
            object_references: false,
            secret_intents: false,
        };
        let required = required_scope(&declared, &custom);
        assert!(
            required.object_references,
            "an under-stated descriptor cannot omit the Read requirement"
        );
        assert!(!required.secret_intents);
    }

    #[test]
    fn operator_endorsed_transport_reaches_a_prepared_batch() {
        use crate::contexts::sync::domain::inventory_export::digest;
        use crate::contexts::sync::domain::inventory_preflight::{
            ExpectedSnapshotAuthority, PreflightProfile, catalog_digest, tool_digest,
        };
        use crate::contexts::sync::infrastructure::providers::capture_snapshot;
        use crate::contexts::sync::infrastructure::providers::immutable_inventory_source::seal_bundle;
        use crate::contexts::sync::infrastructure::providers::netbox_snapshot_capture::tests::transport;

        let provider = Uuid::from_u128(7);
        let mut transport = transport(provider);
        let resources = transport["resources"].as_array_mut().expect("resources");
        let sites = resources
            .iter_mut()
            .find(|resource| resource["resource_key"] == "dcim.sites")
            .expect("site collection");
        sites["rows"] =
            serde_json::json!([{"id": 1, "name": "synthetic-site", "slug": "synthetic-site"}]);
        let bytes = serde_json::to_vec(&transport).expect("transport bytes");
        let capture = capture_snapshot(&bytes, provider, Uuid::from_u128(4), &digest(&bytes))
            .expect("endorsed snapshot");
        let manifest = capture.manifest;
        let payloads = capture.payloads;
        let scope = manifest.declared_scope().expect("scope");
        let attestation = manifest.authority.snapshot.as_ref().expect("attestation");

        let directory = tempfile::tempdir().expect("private bundle");
        let bundle = directory.path().join("bundle");
        let bundle_digest = seal_bundle(&bundle, &manifest, &payloads).expect("seal");
        let config = site_mapping(provider);
        let mapping = serde_json::to_vec(&config).expect("mapping bytes");
        let profile = serde_json::to_vec(&PreflightProfile {
            format_version: 1,
            bundle_digest: bundle_digest.clone(),
            source_provider_id: provider,
            mapping_ref: config.reference,
            mapping_digest: digest(&mapping),
            catalog_digest: catalog_digest(),
            tool_digest: tool_digest(),
            snapshot_authority: Some(ExpectedSnapshotAuthority {
                operator_id: Uuid::from_u128(4),
                provenance_sha256: attestation.provenance.sha256.clone(),
                issuer: attestation.issuer.clone(),
                scope_digest: scope.digest(),
                expectations_digest: attestation.expectations.sha256.clone(),
            }),
        })
        .expect("profile bytes");

        let offline = load_offline(
            &bundle,
            &bundle_digest,
            provider,
            &mapping,
            &profile,
            &digest(&profile),
        )
        .expect("endorsed transport is accepted");
        assert!(offline.report.authority.attested);
        assert_eq!(offline.authority.issuer, "netbox-4.7.1/core-snapshot");
        assert_eq!(offline.artifact_sha256.len(), 64);
        assert!(!offline.batch.rows().is_empty());
    }

    #[test]
    fn scalar_only_rows_acquire_neither_references_nor_a_secret_grant() {
        let plain = batch(
            Uuid::from_u128(2),
            serde_json::json!({"name": "synthetic-site", "slug": "synthetic-site"}),
        );
        let scope = effective_scope(&plain);
        assert!(!scope.custom_fields);
        assert!(!scope.object_references);
        assert!(!scope.secret_intents);
        assert!(!scope_requires_inventory_read(&scope));
        assert!(scope.creates && scope.updates);
    }
}
