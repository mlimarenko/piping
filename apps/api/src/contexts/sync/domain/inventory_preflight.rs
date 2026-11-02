//! Safe deterministic assessment facts. No report authorizes an import.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_export::{
    AuthorityKind, CUSTOM_FIELD_EDGE_PREFIX, SourceAttestationIdentity, digest,
};
use super::inventory_sync::{MappingRef, SourceRecord};
use super::inventory_target_schema::{FieldKind, InventoryEntity};
use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Blocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCode {
    BundleIdentityMismatch,
    MappingDigestMismatch,
    MappingIdentityMismatch,
    ProfileDigestMismatch,
    CatalogIdentityMismatch,
    ToolIdentityMismatch,
    SourceCoverageIncomplete,
    SourceConsistencyUnproven,
    SourceDiscoveryUnproven,
    ProcessCoverageUnobservable,
    UnsupportedEmptyResourceUnused,
    RequestUnsupported,
    MappingInvalid,
    RecordIdentityInvalid,
    ModelUnsupported,
    FieldUnsupported,
    MappingMissing,
    FieldMappingMissing,
    ExpressionInvalid,
    FieldValueInvalid,
    TagsInvalid,
    TagMembershipInvalid,
    TagMembershipDuplicate,
    TagGraphMismatch,
    AnalysisDependencyUnavailable,
    DefinitionInvalid,
    TargetGraphNotAssessed,
    TargetGraphAnalysisIncomplete,
    TargetBindingsInvalid,
    TagDefinitionInvalid,
    TargetDefinitionInvalid,
    TargetRowInvalid,
    TargetReferenceMissing,
    TargetDefinitionConflict,
    CustomFieldScopeInvalid,
    CustomFieldMissing,
    CustomFieldSecret,
    CustomFieldValueInvalid,
    HierarchyInvalid,
    HierarchyCycle,
    /// A cyclic reference no canonical writer can order or defer.
    ReferenceCycleUnorderable,
    /// The supplied profile pins no snapshot authority for this bundle.
    SourceAuthorityUnattested,
    /// The sealed attestation is not the one the caller pinned.
    SourceAuthorityMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    SourceCoverage,
    MappingValidation,
    TargetGraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Remedy {
    SupplyVerifiedInput,
    CaptureAuthoritativeEvidence,
    CorrectMapping,
    ImplementSupport,
    ResolveTargetGraph,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub entity: Option<String>,
    /// Hash rather than arbitrary provider-neutral id text, which may be secret.
    pub record_identity_sha256: Option<String>,
    /// A catalog-owned field, or a hash of an unknown/untrusted field name.
    pub field_path: Option<String>,
    pub affected_count: u64,
    pub remedy: Remedy,
    pub capability: Capability,
    pub edge: Option<EdgeContext>,
    pub component: Option<Vec<GraphNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeContext {
    pub target_entity: Option<String>,
    pub target_identity_sha256: Option<String>,
    pub target_local_id: Option<Uuid>,
}

impl Finding {
    pub fn blocker(code: FindingCode, capability: Capability) -> Self {
        Self {
            code,
            severity: Severity::Blocker,
            entity: None,
            record_identity_sha256: None,
            field_path: None,
            affected_count: 1,
            remedy: match capability {
                Capability::SourceCoverage => Remedy::CaptureAuthoritativeEvidence,
                Capability::MappingValidation => Remedy::CorrectMapping,
                Capability::TargetGraph => Remedy::ResolveTargetGraph,
            },
            capability,
            edge: None,
            component: None,
        }
    }

    pub fn for_record(mut self, record: &SourceRecord) -> Self {
        self.entity = super::inventory_sync::SourceEntity::new(record.identity.entity.as_str())
            .ok()
            .filter(|checked| *checked == record.identity.entity)
            .map(|checked| checked.as_str().into());
        self.record_identity_sha256 = Some(digest(record.identity.external_id.as_bytes()));
        self
    }
}

pub struct PreparationAssessment<B> {
    /// Always None if any blocker exists: a partial batch cannot escape.
    pub batch: Option<B>,
    pub findings: Vec<Finding>,
    pub analysis_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightProfile {
    pub format_version: u32,
    pub bundle_digest: String,
    pub source_provider_id: Uuid,
    pub mapping_ref: MappingRef,
    pub mapping_digest: String,
    pub catalog_digest: String,
    pub tool_digest: String,
    /// Independent pinning of the snapshot attestation. `None` cannot authorize
    /// an import, because a live capture status is never authority.
    pub snapshot_authority: Option<ExpectedSnapshotAuthority>,
}

/// Expected snapshot ownership, exact scope and exact
/// expectations. These values arrive with the profile, whose own digest the
/// caller pins, not from the bundle they are compared against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedSnapshotAuthority {
    pub operator_id: Uuid,
    pub provenance_sha256: String,
    pub issuer: String,
    pub scope_digest: String,
    pub expectations_digest: String,
}

/// The identity includes exact compiled catalog source, not a target row count.
pub fn catalog_digest() -> String {
    let mut bytes = include_bytes!("inventory_target_schema.rs").to_vec();
    bytes.extend_from_slice(include_bytes!("inventory_target_schema/catalog_fields.rs"));
    bytes.extend_from_slice(include_bytes!("inventory_target_schema/hardware_fields.rs"));
    digest(&bytes)
}

/// Exact compiled assessment/importer-owner sources, not a claim about the
/// executable or every transitive dependency. Publication still owns
/// binary/source-SHA proof, which stays a separate identity.
pub fn tool_digest() -> String {
    let mut bytes = include_bytes!("inventory_preflight.rs").to_vec();
    bytes.extend_from_slice(include_bytes!("inventory_export.rs"));
    bytes.extend_from_slice(include_bytes!("inventory_transfer.rs"));
    bytes.extend_from_slice(include_bytes!("../application/inventory_preflight.rs"));
    bytes.extend_from_slice(include_bytes!("../application/inventory_transfer.rs"));
    bytes.extend_from_slice(include_bytes!("../application/mapping_validator.rs"));
    bytes.extend_from_slice(include_bytes!("transform.rs"));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/providers/immutable_inventory_source.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/providers/netbox_snapshot_capture.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/providers/netbox_inventory_source.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../../../../scripts/netbox_snapshot_export.py"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../../../../contracts/netbox-4.7.1-source.json"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target/counters.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target/custom_field_references.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target/interface_graph.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target/tag_memberships.rs"
    ));
    bytes.extend_from_slice(include_bytes!(
        "../infrastructure/persistence/local_inventory_target/transfer_fence.rs"
    ));
    digest(&bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyFact {
    pub from: Uuid,
    pub from_entity: String,
    pub to: Uuid,
    pub to_entity: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityFact {
    pub entity: String,
    pub record_identity_sha256: String,
    pub local_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPlanAssessment {
    /// Transient read observation, `pg_current_snapshot()` text. It is not a
    /// target identity, not a durable fence and is excluded from
    /// [`Self::plan_digest`].
    pub snapshot_identity: String,
    pub snapshot_fingerprint: String,
    pub fingerprint_scope: TargetFingerprintScope,
    pub identities: Vec<IdentityFact>,
    pub dependencies: Vec<DependencyFact>,
    pub findings: Vec<Finding>,
    pub analysis_complete: bool,
    pub cyclic_components: Vec<Vec<GraphNode>>,
}

impl TargetPlanAssessment {
    /// Digest of the exact assessed plan: fingerprint scope and content
    /// identity, identities, dependency edges, findings and cycle verdicts.
    ///
    /// Collections are sorted before hashing, so the digest identifies the plan
    /// rather than a planner iteration order. The transient `snapshot_identity`
    /// is deliberately absent: two reads of the same target produce different
    /// `pg_current_snapshot()` text, while a writer must compare the plan itself
    /// before applying it.
    pub fn plan_digest(&self) -> Result<String, AppError> {
        #[derive(Serialize)]
        struct PlanDigest<'a> {
            fingerprint_scope: &'a TargetFingerprintScope,
            snapshot_fingerprint: &'a str,
            identities: &'a [IdentityFact],
            dependencies: &'a [DependencyFact],
            findings: &'a [Finding],
            analysis_complete: bool,
            cyclic_components: &'a [Vec<GraphNode>],
        }
        let mut identities = self.identities.clone();
        identities.sort_by(|left, right| {
            (
                left.entity.as_str(),
                left.local_id,
                left.record_identity_sha256.as_str(),
            )
                .cmp(&(
                    right.entity.as_str(),
                    right.local_id,
                    right.record_identity_sha256.as_str(),
                ))
        });
        let mut dependencies = self.dependencies.clone();
        dependencies.sort_by(|left, right| {
            (
                left.from_entity.as_str(),
                left.from,
                left.to_entity.as_str(),
                left.to,
                left.field.as_str(),
            )
                .cmp(&(
                    right.from_entity.as_str(),
                    right.from,
                    right.to_entity.as_str(),
                    right.to,
                    right.field.as_str(),
                ))
        });
        let mut findings = self.findings.clone();
        findings.sort();
        let mut cyclic_components = self.cyclic_components.clone();
        cyclic_components.sort();
        let bytes = serde_json::to_vec(&PlanDigest {
            fingerprint_scope: &self.fingerprint_scope,
            snapshot_fingerprint: &self.snapshot_fingerprint,
            identities: &identities,
            dependencies: &dependencies,
            findings: &findings,
            analysis_complete: self.analysis_complete,
            cyclic_components: &cyclic_components,
        })
        .map_err(|_| AppError::Internal("inventory_preflight:plan_digest".into()))?;
        Ok(digest(&bytes))
    }
}

/// UUIDs may be equal in different target tables, especially persisted arbitrary
/// bindings. Graph identity must retain the entity, not assume global UUID keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    pub entity: String,
    pub local_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetFingerprintScope {
    CatalogGraphAndBindings,
}

/// Iterative Kosaraju traversal: deterministic SCCs without recursion over
/// untrusted source-sized graphs. Only actual cyclic components are returned.
pub fn cyclic_components(edges: &[DependencyFact]) -> Vec<Vec<GraphNode>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut graph: BTreeMap<GraphNode, BTreeSet<GraphNode>> = BTreeMap::new();
    let mut reverse: BTreeMap<GraphNode, BTreeSet<GraphNode>> = BTreeMap::new();
    for edge in edges {
        let from = GraphNode {
            entity: edge.from_entity.clone(),
            local_id: edge.from,
        };
        let to = GraphNode {
            entity: edge.to_entity.clone(),
            local_id: edge.to,
        };
        graph.entry(from.clone()).or_default().insert(to.clone());
        graph.entry(to.clone()).or_default();
        reverse.entry(to).or_default().insert(from.clone());
        reverse.entry(from).or_default();
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for root in graph.keys() {
        let mut stack = vec![(root.clone(), false)];
        while let Some((node, exiting)) = stack.pop() {
            if exiting {
                order.push(node);
                continue;
            }
            if !seen.insert(node.clone()) {
                continue;
            }
            stack.push((node.clone(), true));
            for next in graph[&node].iter().rev() {
                if !seen.contains(next) {
                    stack.push((next.clone(), false));
                }
            }
        }
    }
    seen.clear();
    let mut components = Vec::new();
    for root in order.into_iter().rev() {
        if seen.contains(&root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            component.push(node.clone());
            for next in reverse[&node].iter().rev() {
                if !seen.contains(next) {
                    stack.push(next.clone());
                }
            }
        }
        component.sort_unstable();
        if component.len() > 1 || graph[&root].contains(&root) {
            components.push(component);
        }
    }
    components.sort();
    components
}

/// Classify the ordered part of the dependency graph.
///
/// The canonical writer withholds every legally deferable reference — nullable
/// and junction-backed columns plus JSON custom-field edges — then orders the
/// rows that remain and applies the withheld edges once every identity exists.
/// Only the remaining required graph must therefore be acyclic: a cycle that
/// disappears when one nullable edge is withheld is lawful, even if its other
/// edges stay in the first pass.
///
/// A cycle that survives on the required graph cannot be inserted in any order.
/// Nested-set parent cycles are the canonical planner's `HierarchyCycle`, so a
/// residual cycle whose nodes are already covered by one is not re-reported;
/// a required cycle that merely *includes* a parent edge is not covered by that
/// verdict and stays a blocker here.
pub(crate) fn future_two_pass_findings(
    edges: &[DependencyFact],
    identities: &[IdentityFact],
) -> Vec<Finding> {
    use std::collections::BTreeSet;

    let required: Vec<DependencyFact> = edges
        .iter()
        .filter(|edge| !deferrable_edge(edge))
        .cloned()
        .collect();
    // Nested-set cycles are the planner's `HierarchyCycle`; a required cycle
    // that merely *includes* a parent edge is not explained by that verdict.
    let hierarchy: Vec<DependencyFact> = required
        .iter()
        .filter(|edge| hierarchy_edge(edge))
        .cloned()
        .collect();
    let explained: Vec<BTreeSet<GraphNode>> = cyclic_components(&hierarchy)
        .into_iter()
        .map(|component| component.into_iter().collect())
        .collect();
    let mut findings = Vec::new();
    for component in cyclic_components(&required) {
        let members: BTreeSet<_> = component.iter().cloned().collect();
        if explained.iter().any(|cycle| members.is_subset(cycle)) {
            continue;
        }
        let mut finding = Finding::blocker(
            FindingCode::ReferenceCycleUnorderable,
            Capability::TargetGraph,
        );
        if let Some(first) = component.first() {
            finding.entity = Some(first.entity.clone());
            finding.record_identity_sha256 = identities
                .iter()
                .find(|identity| {
                    identity.entity == first.entity && identity.local_id == first.local_id
                })
                .map(|identity| identity.record_identity_sha256.clone());
        }
        finding.field_path = Some("dependencies.scc".into());
        finding.affected_count = component.len() as u64;
        finding.component = Some(component);
        findings.push(finding);
    }
    findings
}

/// A nested-set parent edge of a tree resource. A required `parent` reference on
/// any other resource is an ordinary reference, not hierarchy bookkeeping.
pub(crate) fn hierarchy_edge(edge: &DependencyFact) -> bool {
    matches!(edge.field.as_str(), "parent" | "parent_id")
        && InventoryEntity::from_source_key(&edge.from_entity).is_some_and(InventoryEntity::is_tree)
}

/// Whether one target edge may be withheld from the first insert pass.
///
/// The answer comes from the closed catalog, not from the edge label alone, so
/// it cannot drift from the tables and columns the writer actually defers.
pub(crate) fn deferrable_edge(edge: &DependencyFact) -> bool {
    if hierarchy_edge(edge) {
        return false;
    }
    let Some(entity) = InventoryEntity::from_source_key(&edge.from_entity) else {
        return false;
    };
    if edge.field.starts_with(CUSTOM_FIELD_EDGE_PREFIX) {
        // JSON is installed after all rows even on hardware/tree resources;
        // withholding it does not relax their FK or hierarchy invariants.
        return entity.has_custom_field_data();
    }
    let Some(field) = entity.field(edge_field_name(&edge.field)) else {
        return false;
    };
    if !entity.allows_deferred_references()
        && !(entity == InventoryEntity::InterfaceTemplate && field.name() == "bridge")
    {
        return false;
    }
    match field.kind() {
        FieldKind::ForeignKey { .. } | FieldKind::GenericReference { .. } => field.nullable(),
        FieldKind::Junction { .. } | FieldKind::ContentTypeJunction { .. } => true,
        FieldKind::Tags | FieldKind::Scalar => false,
    }
}

/// Source field name without the `[index]` suffix of a junction entry.
fn edge_field_name(field: &str) -> &str {
    match field.find('[') {
        Some(index) if field.ends_with(']') => field[..index].trim_end(),
        _ => field,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentOutcome {
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportCapability {
    Unavailable,
}

/// Sanitized verdict about the sealed authority claim this report compared with
/// the independently supplied profile. It exposes digests and no path or value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEvidence {
    pub kind: AuthorityKind,
    /// Whether the supplied profile pinned exactly this attestation.
    pub attested: bool,
    /// Verified attestation identity, present only when the pinned profile
    /// matched. A descriptor must copy these values; it cannot establish them.
    pub attestation: Option<SourceAttestationIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreflightReport {
    pub format_version: u32,
    pub outcome: AssessmentOutcome,
    pub import_capability: ImportCapability,
    pub bundle_digest: String,
    pub source_provider_id: Uuid,
    pub mapping_digest: String,
    pub mapping_ref: MappingRef,
    pub profile_digest: String,
    pub catalog_digest: String,
    pub tool_version: String,
    pub tool_digest: String,
    pub resource_count: u64,
    pub observed_rows: u64,
    pub source_coverage_complete: bool,
    pub offline_analysis_complete: bool,
    pub authority: AuthorityEvidence,
    pub target: Option<TargetPlanAssessment>,
    /// [`TargetPlanAssessment::plan_digest`] of `target`, for exact replay checks.
    pub plan_digest: Option<String>,
    pub findings: Vec<Finding>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sccs_are_deterministic_and_include_self_cycles_only_when_cyclic() {
        let edges = [(1, 2), (2, 1), (2, 3), (4, 4), (5, 6)].map(|(a, b)| DependencyFact {
            from: Uuid::from_u128(a),
            from_entity: "synthetic.nodes".into(),
            to: Uuid::from_u128(b),
            to_entity: "synthetic.nodes".into(),
            field: "reference".into(),
        });
        let node = |id| GraphNode {
            entity: "synthetic.nodes".into(),
            local_id: Uuid::from_u128(id),
        };
        let expected = vec![vec![node(1), node(2)], vec![node(4)]];
        assert_eq!(cyclic_components(&edges), expected);
        let mut reversed = edges.to_vec();
        reversed.reverse();
        assert_eq!(cyclic_components(&reversed), expected);
    }

    #[test]
    fn arbitrary_uuid_equality_across_target_entities_is_not_a_self_cycle() {
        let edge = DependencyFact {
            from: Uuid::from_u128(1),
            from_entity: "dcim.devices".into(),
            to: Uuid::from_u128(1),
            to_entity: "ipam.ip-addresses".into(),
            field: "primary_ip4".into(),
        };
        assert!(cyclic_components(std::slice::from_ref(&edge)).is_empty());
        let reverse = DependencyFact {
            from: edge.to,
            from_entity: edge.to_entity.clone(),
            to: edge.from,
            to_entity: edge.from_entity.clone(),
            field: "assigned_object".into(),
        };
        let components = cyclic_components(&[edge, reverse]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 2);
    }

    #[test]
    fn independent_equal_sized_scc_findings_keep_distinct_context_and_exclude_hierarchies() {
        let edges: Vec<_> = [
            (1, 2, "synthetic.nodes", "reference"),
            (2, 1, "synthetic.nodes", "reference"),
            (3, 4, "synthetic.nodes", "reference"),
            (4, 3, "synthetic.nodes", "reference"),
            // A nested-set parent self-cycle on a real tree resource.
            (5, 5, "dcim.regions", "parent"),
        ]
        .into_iter()
        .map(|(from, to, entity, field)| edge(from, entity, to, entity, field))
        .collect();
        let mut findings = future_two_pass_findings(&edges, &[]);
        findings.sort();
        findings.dedup();
        // The parent self-cycle stays with the canonical planner; the two
        // reference cycles name an unknown entity, so neither is deferrable.
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|finding| finding.code
            == FindingCode::ReferenceCycleUnorderable
            && finding.capability == Capability::TargetGraph));
        assert!(findings.iter().all(|finding| {
            finding.affected_count == 2
                && finding
                    .component
                    .as_ref()
                    .is_some_and(|component| component.len() == 2)
        }));
        assert_ne!(findings[0].component, findings[1].component);
        assert_eq!(cyclic_components(&edges).len(), 3);
    }

    fn edge(
        from: u128,
        from_entity: &str,
        to: u128,
        to_entity: &str,
        field: &str,
    ) -> DependencyFact {
        DependencyFact {
            from: Uuid::from_u128(from),
            from_entity: from_entity.into(),
            to: Uuid::from_u128(to),
            to_entity: to_entity.into(),
            field: field.into(),
        }
    }

    fn node_edge(from: u128, to: u128, entity: &str, field: &str) -> DependencyFact {
        edge(from, entity, to, entity, field)
    }

    #[test]
    fn lawful_deferred_cycles_pass_while_required_and_hierarchy_cycles_block() {
        // Two nullable catalog references between tenants: the canonical writer
        // inserts both rows, then applies both references.
        let deferrable = [
            node_edge(1, 2, "tenancy.tenants", "group"),
            node_edge(2, 1, "tenancy.tenants", "group"),
        ];
        assert!(deferrable.iter().all(deferrable_edge));
        assert!(future_two_pass_findings(&deferrable, &[]).is_empty());

        // Object/multiobject custom-field JSON edges are deferred as well.
        let custom_fields = [
            node_edge(1, 2, "tenancy.tenants", "custom_fields.sha256:aa"),
            node_edge(2, 1, "tenancy.tenants", "custom_fields.sha256:bb"),
        ];
        assert!(future_two_pass_findings(&custom_fields, &[]).is_empty());

        // A required column cannot be withheld from the first insert pass.
        let required = [
            node_edge(1, 2, "dcim.device-types", "manufacturer"),
            node_edge(2, 1, "dcim.device-types", "manufacturer"),
        ];
        assert!(required.iter().all(|fact| !deferrable_edge(fact)));
        let findings = future_two_pass_findings(&required, &[]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, FindingCode::ReferenceCycleUnorderable);
        assert_eq!(findings[0].affected_count, 2);

        // The protected device/interface/cable graph is ordered natively: even a
        // nullable reference cycle there is not deferrable.
        let hardware = [
            node_edge(1, 2, "dcim.interfaces", "lag"),
            node_edge(2, 1, "dcim.interfaces", "lag"),
        ];
        assert!(hardware.iter().all(|fact| !deferrable_edge(fact)));
        assert_eq!(future_two_pass_findings(&hardware, &[]).len(), 1);

        // A pure hierarchy cycle stays with the canonical planner, which reports
        // `HierarchyCycle`; this classification never claims two-pass repairs it.
        let hierarchy = [
            node_edge(1, 2, "dcim.regions", "parent"),
            node_edge(2, 1, "dcim.regions", "parent"),
        ];
        assert!(future_two_pass_findings(&hierarchy, &[]).is_empty());
        assert_eq!(cyclic_components(&hierarchy).len(), 1);

        // A parent chain with a JSON reference back is not a hierarchy cycle:
        // only the JSON edge is withheld, and the parent order is preserved.
        let mixed = [
            node_edge(1, 2, "dcim.regions", "parent"),
            node_edge(2, 1, "dcim.regions", "custom_fields.sha256:cc"),
        ];
        assert!(future_two_pass_findings(&mixed, &[]).is_empty());
        let device_self_reference = [node_edge(1, 1, "dcim.devices", "custom_fields.sha256:dd")];
        assert!(future_two_pass_findings(&device_self_reference, &[]).is_empty());
    }

    #[test]
    fn mixed_cycle_is_orderable_once_its_nullable_edges_are_withheld() {
        // VM → primary IP → VM interface → required VM. The last edge stays in
        // the first insert pass; awaiting the two nullable ones does not make
        // the cycle impossible.
        let edges = [
            edge(
                1,
                "virtualization.virtual-machines",
                2,
                "ipam.ip-addresses",
                "primary_ip4",
            ),
            edge(
                2,
                "ipam.ip-addresses",
                3,
                "virtualization.interfaces",
                "assigned_object",
            ),
            edge(
                3,
                "virtualization.interfaces",
                1,
                "virtualization.virtual-machines",
                "virtual_machine",
            ),
        ];
        assert_eq!(cyclic_components(&edges).len(), 1);
        assert!(deferrable_edge(&edges[0]) && deferrable_edge(&edges[1]));
        assert!(!deferrable_edge(&edges[2]));
        assert!(future_two_pass_findings(&edges, &[]).is_empty());
    }

    #[test]
    fn mixed_cycle_whose_required_part_still_cycles_stays_blocked() {
        // The custom-field edge is withheld, but the required generic parent
        // references still form a cycle no insertion order can satisfy.
        let edges = [
            edge(1, "ipam.services", 2, "ipam.services", "parent"),
            edge(2, "ipam.services", 1, "ipam.services", "parent"),
            edge(
                1,
                "ipam.services",
                2,
                "ipam.services",
                "custom_fields.sha256:aa",
            ),
        ];
        assert!(deferrable_edge(&edges[2]));
        assert!(!deferrable_edge(&edges[0]));
        let findings = future_two_pass_findings(&edges, &[]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, FindingCode::ReferenceCycleUnorderable);
        assert_eq!(findings[0].affected_count, 2);
    }

    #[test]
    fn plan_digest_covers_the_plan_and_ignores_the_transient_snapshot_identity() {
        let mut plan = TargetPlanAssessment {
            snapshot_identity: "1:2:3".into(),
            snapshot_fingerprint: "a".repeat(64),
            fingerprint_scope: TargetFingerprintScope::CatalogGraphAndBindings,
            identities: vec![
                IdentityFact {
                    entity: "dcim.devices".into(),
                    record_identity_sha256: "b".repeat(64),
                    local_id: Uuid::from_u128(1),
                },
                IdentityFact {
                    entity: "dcim.devices".into(),
                    record_identity_sha256: "d".repeat(64),
                    local_id: Uuid::from_u128(2),
                },
            ],
            dependencies: vec![
                node_edge(1, 2, "dcim.devices", "reference"),
                node_edge(2, 1, "dcim.devices", "reference"),
            ],
            findings: Vec::new(),
            analysis_complete: true,
            cyclic_components: Vec::new(),
        };
        let expected = plan.plan_digest().expect("plan digest");
        plan.snapshot_identity = "9:9:9".into();
        assert_eq!(plan.plan_digest().expect("repeated digest"), expected);
        let mut reordered = plan.clone();
        reordered.identities.reverse();
        reordered.dependencies.reverse();
        assert_eq!(
            reordered.plan_digest().expect("reordered digest"),
            expected,
            "planner iteration order must not change plan identity"
        );

        for change in 0..5 {
            let mut changed = plan.clone();
            match change {
                0 => changed.snapshot_fingerprint = "c".repeat(64),
                1 => changed.identities.clear(),
                2 => changed.dependencies.clear(),
                3 => changed.analysis_complete = false,
                _ => changed.findings.push(Finding::blocker(
                    FindingCode::TargetGraphNotAssessed,
                    Capability::TargetGraph,
                )),
            }
            assert_ne!(
                changed.plan_digest().expect("changed digest"),
                expected,
                "change {change} must alter the plan identity"
            );
        }
    }
}
