//! Offline assessment never constructs a pool, Redis client or application state.
//! Only the separate dry-run function receives an actual SQL target context.

use crate::contexts::sync::domain::inventory_export::{
    AuthorityKind, CaptureOutcome, CaptureStatus, ConsistencyStatus, ObservationStatus, digest,
};
use crate::contexts::sync::domain::inventory_preflight::{
    AssessmentOutcome, AuthorityEvidence, Capability, Finding, FindingCode, ImportCapability,
    PreflightProfile, PreflightReport, Remedy, Severity, catalog_digest, tool_digest,
};
use crate::contexts::sync::domain::{InventoryEntity, MappingConfigDocument};
use crate::contexts::sync::infrastructure::persistence::local_inventory_target::{
    LocalInventoryBatch, LocalInventoryCatalog, assess_preparation_from_source,
};
use crate::contexts::sync::infrastructure::providers::ImmutableInventorySource;
use crate::errors::AppError;

pub struct OfflineAssessment {
    pub report: PreflightReport,
    batch: Option<LocalInventoryBatch>,
}

impl OfflineAssessment {
    /// Lets callers reject invalid input before reading any database settings.
    pub fn can_plan_target(&self) -> bool {
        self.batch.is_some()
    }
}

/// Hash actual supplied mapping/profile bytes, not a reserialization of serde
/// values. The expected profile digest must arrive independently of those bytes.
/// There is deliberately no database configuration or target parameter here.
pub fn preflight(
    source: &ImmutableInventorySource,
    mapping_bytes: &[u8],
    profile_bytes: &[u8],
    expected_profile_digest: &str,
) -> Result<OfflineAssessment, AppError> {
    let config: MappingConfigDocument = serde_json::from_slice(mapping_bytes)
        .map_err(|_| AppError::Validation("inventory_preflight:mapping_encoding".into()))?;
    let profile: PreflightProfile = serde_json::from_slice(profile_bytes)
        .map_err(|_| AppError::Validation("inventory_preflight:profile_encoding".into()))?;
    let manifest = source.manifest();
    let mapping_digest = digest(mapping_bytes);
    let profile_digest = digest(profile_bytes);
    let catalog_digest = catalog_digest();
    let tool_digest = tool_digest();
    let mut findings = Vec::new();
    for (invalid, code) in [
        (
            profile.format_version != 1 || profile_digest != expected_profile_digest,
            FindingCode::ProfileDigestMismatch,
        ),
        (
            profile.bundle_digest != source.bundle_digest()
                || profile.source_provider_id != manifest.source_provider_id,
            FindingCode::BundleIdentityMismatch,
        ),
        (
            profile.mapping_digest != mapping_digest,
            FindingCode::MappingDigestMismatch,
        ),
        (
            profile.mapping_ref != config.reference
                || config.source_provider_id != manifest.source_provider_id
                || config.reference.version_id <= 0
                || config.reference.logical_id.is_nil(),
            FindingCode::MappingIdentityMismatch,
        ),
        (
            profile.catalog_digest != catalog_digest,
            FindingCode::CatalogIdentityMismatch,
        ),
        (
            profile.tool_digest != tool_digest,
            FindingCode::ToolIdentityMismatch,
        ),
    ] {
        if invalid {
            findings.push(Finding::blocker(code, Capability::MappingValidation));
        }
    }
    if manifest.capture.outcome != CaptureOutcome::Complete {
        findings.push(Finding::blocker(
            FindingCode::SourceCoverageIncomplete,
            Capability::SourceCoverage,
        ));
    }
    if manifest.capture.consistency != ConsistencyStatus::Consistent {
        findings.push(Finding::blocker(
            FindingCode::SourceConsistencyUnproven,
            Capability::SourceCoverage,
        ));
    }
    if !manifest.capture.discovery_complete {
        findings.push(Finding::blocker(
            FindingCode::SourceDiscoveryUnproven,
            Capability::SourceCoverage,
        ));
    }
    // Capture status describes a read. Only an attestation the profile pins
    // independently establishes that this exact bundle is owned and trusted.
    let declared_scope_digest = manifest.declared_scope().map(|scope| scope.digest()).ok();
    let attested = match (
        profile.snapshot_authority.as_ref(),
        &manifest.authority.snapshot,
    ) {
        (Some(expected), Some(attestation)) => {
            manifest.authority.kind == AuthorityKind::OperatorEndorsedSnapshot
                && attestation.operator_id == expected.operator_id
                && attestation.provenance.sha256 == expected.provenance_sha256
                && attestation.issuer == expected.issuer
                && declared_scope_digest.as_deref() == Some(expected.scope_digest.as_str())
                && attestation.expectations.sha256 == expected.expectations_digest
        }
        _ => false,
    };
    if profile.snapshot_authority.is_none() {
        findings.push(Finding::blocker(
            FindingCode::SourceAuthorityUnattested,
            Capability::SourceCoverage,
        ));
    } else if !attested {
        let mut finding = Finding::blocker(
            FindingCode::SourceAuthorityMismatch,
            Capability::SourceCoverage,
        );
        finding.remedy = Remedy::SupplyVerifiedInput;
        findings.push(finding);
    }
    let authority = AuthorityEvidence {
        kind: manifest.authority.kind,
        attested,
        // Derived from the verified fields only; unpinned input is never exposed
        // as a pinnable identity.
        attestation: if attested {
            manifest.authority.attestation_identity()?
        } else {
            None
        },
    };
    for resource in &manifest.resources {
        if resource.status != CaptureStatus::Captured {
            let mut finding = Finding::blocker(
                FindingCode::SourceCoverageIncomplete,
                Capability::SourceCoverage,
            );
            finding.entity = resource.entity.as_ref().map(|e| e.as_str().into());
            // Unsupported paths may contain credentials; identify them only by
            // digest, never the raw resource_key/endpoint from discovery.
            finding.record_identity_sha256 = Some(digest(resource.resource_key.as_bytes()));
            findings.push(finding);
        } else if resource.observed_rows == 0
            && resource
                .entity
                .as_ref()
                .is_some_and(|e| InventoryEntity::from_source_key(e.as_str()).is_none())
        {
            let mut finding = Finding::blocker(
                FindingCode::UnsupportedEmptyResourceUnused,
                Capability::MappingValidation,
            );
            finding.entity = resource.entity.as_ref().map(|e| e.as_str().into());
            finding.severity = Severity::Info;
            finding.affected_count = 0;
            findings.push(finding);
        }
    }
    for process in &manifest.processes {
        if process.status != ObservationStatus::Observed {
            let mut finding = Finding::blocker(
                FindingCode::ProcessCoverageUnobservable,
                Capability::SourceCoverage,
            );
            finding.field_path = Some(match process.area {
                crate::contexts::sync::domain::inventory_export::ProcessArea::Plugins => "process.plugins",
                crate::contexts::sync::domain::inventory_export::ProcessArea::ScriptsAndJobs => "process.scripts_and_jobs",
                crate::contexts::sync::domain::inventory_export::ProcessArea::Webhooks => "process.webhooks",
                crate::contexts::sync::domain::inventory_export::ProcessArea::EventRules => "process.event_rules",
                crate::contexts::sync::domain::inventory_export::ProcessArea::Permissions => "process.permissions",
            }.into());
            findings.push(finding);
        }
    }
    let source_coverage_complete = !findings
        .iter()
        .any(|f| f.capability == Capability::SourceCoverage && f.severity == Severity::Blocker);
    let validation = assess_preparation_from_source(&config, &source);
    let offline_analysis_complete = validation.analysis_complete;
    findings.extend(validation.findings);
    findings.push(Finding::blocker(
        FindingCode::TargetGraphNotAssessed,
        Capability::TargetGraph,
    ));
    findings.sort();
    let identity_mismatch = findings.iter().any(|f| {
        matches!(
            f.code,
            FindingCode::BundleIdentityMismatch
                | FindingCode::MappingDigestMismatch
                | FindingCode::MappingIdentityMismatch
                | FindingCode::ProfileDigestMismatch
                | FindingCode::CatalogIdentityMismatch
                | FindingCode::ToolIdentityMismatch
        )
    });
    Ok(OfflineAssessment {
        batch: if identity_mismatch {
            None
        } else {
            validation.batch
        },
        report: PreflightReport {
            format_version: 1,
            outcome: AssessmentOutcome::Completed,
            import_capability: ImportCapability::Unavailable,
            bundle_digest: source.bundle_digest().into(),
            source_provider_id: manifest.source_provider_id,
            mapping_digest,
            mapping_ref: config.reference,
            profile_digest,
            catalog_digest,
            tool_version: env!("CARGO_PKG_VERSION").into(),
            tool_digest,
            resource_count: manifest.resources.len() as u64,
            observed_rows: manifest.resources.iter().map(|r| r.observed_rows).sum(),
            source_coverage_complete,
            offline_analysis_complete,
            authority,
            target: None,
            plan_digest: None,
            findings,
        },
    })
}

/// Read-only graph observation may still be useful for an incomplete source.
/// It cannot run with a validation-invalid partial batch or mismatched inputs.
pub async fn dry_run(
    target: &LocalInventoryCatalog,
    mut assessment: OfflineAssessment,
) -> Result<PreflightReport, AppError> {
    if let Some(batch) = assessment.batch {
        let plan = target.plan_read_only(&batch).await?;
        assessment
            .report
            .findings
            .retain(|f| f.code != FindingCode::TargetGraphNotAssessed);
        assessment.report.findings.extend(plan.findings.clone());
        if !plan.analysis_complete {
            assessment.report.findings.push(Finding::blocker(
                FindingCode::TargetGraphAnalysisIncomplete,
                Capability::TargetGraph,
            ));
        }
        assessment.report.plan_digest = Some(plan.plan_digest()?);
        assessment.report.target = Some(plan);
        assessment.report.findings.sort();
    }
    Ok(assessment.report)
}
