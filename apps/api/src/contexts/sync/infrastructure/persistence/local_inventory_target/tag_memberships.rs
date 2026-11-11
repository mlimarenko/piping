use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_export::digest;
use crate::contexts::sync::domain::inventory_preflight::{
    Capability, EdgeContext, Finding, FindingCode,
};
use crate::contexts::sync::domain::{InventoryEntity, SourceSnapshot};
use crate::contexts::sync::infrastructure::providers::immutable_inventory_source::VerifiedTagRelationships;

use super::PreparationFacts;

fn external_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty() && text.trim() == *text)
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

/// NetBox embeds Tag objects. Display names and slugs are not identities.
pub(super) fn embedded_id(value: &Value) -> Option<String> {
    value.as_object()?.get("id").and_then(external_id)
}

fn owner_finding(owner: &(InventoryEntity, String), field: &str) -> Finding {
    let mut finding = Finding::blocker(
        FindingCode::AnalysisDependencyUnavailable,
        Capability::MappingValidation,
    );
    finding.entity = Some(owner.0.source_key().into());
    finding.record_identity_sha256 = Some(digest(owner.1.as_bytes()));
    finding.field_path = Some(field.into());
    finding
}

fn mismatch(owner: &(InventoryEntity, String), tag_id: &str) -> Finding {
    let mut finding = owner_finding(owner, "tags");
    finding.code = FindingCode::TagGraphMismatch;
    finding.edge = Some(EdgeContext {
        target_entity: Some(InventoryEntity::Tag.source_key().into()),
        target_identity_sha256: Some(digest(tag_id.as_bytes())),
        target_local_id: None,
    });
    finding
}

/// Compare each independently observable owner, not the success of unrelated
/// rows. Unknown owner identities prevent claims about absent membership edges,
/// but cannot erase a positively observed edge missing from a known tag list.
pub(super) fn assess(
    snapshot: &SourceSnapshot,
    facts: &[PreparationFacts],
    provider_id: Uuid,
) -> (Vec<Finding>, bool) {
    assess_with_evidence(snapshot, facts, provider_id, None)
}

pub(super) fn assess_with_evidence(
    snapshot: &SourceSnapshot,
    facts: &[PreparationFacts],
    provider_id: Uuid,
    evidence: Option<&VerifiedTagRelationships>,
) -> (Vec<Finding>, bool) {
    let mut findings = Vec::new();
    let mut complete = true;
    let tag_ids: BTreeSet<_> = facts
        .iter()
        .filter(|fact| fact.entity == InventoryEntity::Tag)
        .map(|fact| fact.identity.external_id.as_str())
        .collect();
    // Inspect every embedded entry independently, even when another item or
    // unrelated mapped field prevented a writable row from being prepared.
    for record in &snapshot.records {
        let Some(tags) = record.fields.get("tags") else {
            continue;
        };
        let mut seen = BTreeSet::new();
        let Some(items) = tags.as_array() else {
            let mut finding =
                Finding::blocker(FindingCode::TagsInvalid, Capability::MappingValidation)
                    .for_record(record);
            finding.field_path = Some("tags".into());
            findings.push(finding);
            continue;
        };
        for (index, item) in items.iter().enumerate() {
            let id = embedded_id(item);
            let code = match id.as_deref() {
                None => Some(FindingCode::TagsInvalid),
                Some(id) if !seen.insert(id.to_owned()) => {
                    Some(FindingCode::TagMembershipDuplicate)
                }
                Some(id)
                    if record.identity.source_provider_id != provider_id
                        || !tag_ids.contains(id) =>
                {
                    Some(FindingCode::TargetReferenceMissing)
                }
                Some(_) => None,
            };
            if let Some(code) = code {
                let mut finding =
                    Finding::blocker(code, Capability::MappingValidation).for_record(record);
                finding.field_path = Some(format!("tags[{index}]"));
                finding.edge = id.map(|id| EdgeContext {
                    target_entity: Some(InventoryEntity::Tag.source_key().into()),
                    target_identity_sha256: Some(digest(id.as_bytes())),
                    target_local_id: None,
                });
                findings.push(finding);
            }
        }
    }
    if let Some(evidence) = evidence {
        if !evidence.matches(snapshot, provider_id) {
            let mut finding = Finding::blocker(
                FindingCode::AnalysisDependencyUnavailable,
                Capability::MappingValidation,
            );
            finding.field_path = Some("tags.graph.authority".into());
            findings.push(finding);
            return (findings, false);
        }
        // Independent TaggedObject rows, when present, still require the
        // ordinary graph guard rather than replacing one source with another.
        if !snapshot.records.iter().any(|record| {
            record.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key()
        }) {
            let actual: BTreeMap<_, _> = facts
                .iter()
                .map(|fact| {
                    (
                        (
                            fact.entity.source_key().to_owned(),
                            fact.identity.external_id.clone(),
                        ),
                        fact,
                    )
                })
                .collect();
            let owners: BTreeSet<_> = actual
                .keys()
                .chain(evidence.edges().keys())
                .cloned()
                .collect();
            for key in owners {
                let Some(entity) = InventoryEntity::from_source_key(&key.0) else {
                    continue;
                };
                let owner = (entity, key.1.clone());
                let empty = BTreeSet::new();
                let wanted = evidence.edges().get(&key).unwrap_or(&empty);
                let Some(fact) = actual.get(&key) else {
                    findings.push(owner_finding(&owner, "tags.graph.actual"));
                    complete = false;
                    continue;
                };
                let Some(tags) = &fact.tags else {
                    if !wanted.is_empty() || fact.tags_relevant {
                        findings.push(owner_finding(&owner, "tags.graph.actual"));
                        complete = false;
                    }
                    continue;
                };
                let observed: BTreeSet<_> = tags.iter().cloned().collect();
                for id in wanted.symmetric_difference(&observed) {
                    findings.push(mismatch(&owner, id));
                }
            }
            findings.sort();
            findings.dedup();
            return (findings, complete);
        }
    }
    let mut identity_counts = BTreeMap::new();
    for record in snapshot.records.iter().filter(|record| {
        record.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key()
    }) {
        *identity_counts
            .entry((
                record.identity.source_provider_id,
                record.identity.external_id.clone(),
            ))
            .or_insert(0usize) += 1;
    }
    let mut memberships = BTreeMap::<_, Vec<_>>::new();
    let mut expected = BTreeMap::<_, BTreeSet<String>>::new();
    let mut uncertain_owners = BTreeSet::new();
    let mut unknown_owner = false;
    for record in snapshot.records.iter().filter(|record| {
        record.identity.entity.as_str() == InventoryEntity::TaggedObject.source_key()
    }) {
        let entity = record
            .fields
            .get("object_type")
            .and_then(Value::as_str)
            .and_then(InventoryEntity::from_content_type_token);
        let object_id = record.fields.get("object_id").and_then(external_id);
        let tag_id = record.fields.get("tag").and_then(embedded_id);
        for (invalid, field) in [
            (entity.is_none(), "object_type"),
            (object_id.is_none(), "object_id"),
            (tag_id.is_none(), "tag"),
        ] {
            if invalid {
                let mut finding = Finding::blocker(
                    FindingCode::TagMembershipInvalid,
                    Capability::MappingValidation,
                )
                .for_record(record);
                finding.field_path = Some(field.into());
                findings.push(finding);
                complete = false;
            }
        }
        if let (Some(entity), Some(object_id), Some(tag_id)) = (entity, &object_id, &tag_id) {
            memberships
                .entry((
                    record.identity.source_provider_id,
                    entity,
                    object_id.clone(),
                    tag_id.clone(),
                ))
                .or_default()
                .push(record);
        }
        if record.identity.source_provider_id != provider_id
            || identity_counts.get(&(
                record.identity.source_provider_id,
                record.identity.external_id.clone(),
            )) != Some(&1)
        {
            let mut finding = Finding::blocker(
                FindingCode::AnalysisDependencyUnavailable,
                Capability::MappingValidation,
            )
            .for_record(record);
            finding.field_path = Some("identity".into());
            findings.push(finding);
            unknown_owner = true;
            complete = false;
            continue;
        }
        let (Some(entity), Some(object_id)) = (entity, object_id) else {
            unknown_owner = true;
            complete = false;
            continue;
        };
        let owner = (entity, object_id);
        let Some(tag_id) = tag_id else {
            uncertain_owners.insert(owner);
            continue;
        };
        if !tag_ids.contains(tag_id.as_str()) {
            let mut finding = Finding::blocker(
                FindingCode::AnalysisDependencyUnavailable,
                Capability::MappingValidation,
            )
            .for_record(record);
            finding.field_path = Some("tag".into());
            finding.edge = Some(EdgeContext {
                target_entity: Some(InventoryEntity::Tag.source_key().into()),
                target_identity_sha256: Some(digest(tag_id.as_bytes())),
                target_local_id: None,
            });
            findings.push(finding);
            uncertain_owners.insert(owner);
            complete = false;
            continue;
        }
        expected.entry(owner).or_default().insert(tag_id);
    }
    for ((_, _, _, tag_id), records) in memberships
        .into_iter()
        .filter(|(_, records)| records.len() > 1)
    {
        for record in &records {
            let mut finding = Finding::blocker(
                FindingCode::TagMembershipDuplicate,
                Capability::MappingValidation,
            )
            .for_record(record);
            finding.field_path = Some("tag".into());
            finding.affected_count = records.len() as u64;
            finding.edge = Some(EdgeContext {
                target_entity: Some(InventoryEntity::Tag.source_key().into()),
                target_identity_sha256: Some(digest(tag_id.as_bytes())),
                target_local_id: None,
            });
            findings.push(finding);
        }
    }
    let actual: BTreeMap<_, _> = facts
        .iter()
        .map(|fact| {
            (
                (fact.entity, fact.identity.external_id.clone()),
                (
                    fact.tags
                        .as_ref()
                        .map(|tags| tags.iter().cloned().collect::<BTreeSet<_>>()),
                    fact.tags_relevant,
                ),
            )
        })
        .collect();
    let owners: BTreeSet<_> = actual
        .keys()
        .chain(expected.keys())
        .chain(uncertain_owners.iter())
        .cloned()
        .collect();
    for owner in owners {
        let empty = BTreeSet::new();
        let wanted = expected.get(&owner).unwrap_or(&empty);
        let observed = actual.get(&owner);
        // Omission preserves the stored array only when no independent edge
        // requires an observation we cannot compare with that array here.
        let Some((Some(tags), _)) = observed else {
            if !wanted.is_empty()
                || uncertain_owners.contains(&owner)
                || observed.is_some_and(|(_, relevant)| *relevant)
            {
                findings.push(owner_finding(&owner, "tags.graph.actual"));
                complete = false;
            }
            continue;
        };
        for tag_id in wanted.difference(tags) {
            findings.push(mismatch(&owner, tag_id));
        }
        if !unknown_owner && !uncertain_owners.contains(&owner) {
            for tag_id in tags.difference(wanted) {
                findings.push(mismatch(&owner, tag_id));
            }
        } else {
            findings.push(owner_finding(&owner, "tags.graph.expected"));
            complete = false;
        }
    }
    if unknown_owner {
        let mut finding = Finding::blocker(
            FindingCode::AnalysisDependencyUnavailable,
            Capability::MappingValidation,
        );
        finding.field_path = Some("tags.graph.owners".into());
        findings.push(finding);
    }
    findings.sort();
    findings.dedup();
    (findings, complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::sync::infrastructure::providers::{
        ImmutableInventorySource,
        netbox_snapshot_capture::{capture_snapshot, tests::transport},
    };
    use serde_json::json;

    #[test]
    fn evidence_refuses_another_provider_or_snapshot() {
        let provider = Uuid::now_v7();
        let mut transport = transport(provider);
        let site = transport["resources"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|resource| resource["resource_key"] == "dcim.sites")
            .unwrap();
        site["rows"] = json!([{"id": 1, "name": "Site", "slug": "site", "tags": []}]);
        let bytes = serde_json::to_vec(&transport).unwrap();
        let capture = capture_snapshot(&bytes, provider, Uuid::now_v7(), &digest(&bytes)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("snapshot");
        let anchor = capture.seal(&bundle).unwrap();
        let source = ImmutableInventorySource::open(&bundle, &anchor, provider, 100).unwrap();
        let evidence = source
            .verified_tag_relationships()
            .expect("canonical evidence");
        let mut snapshot = source.snapshot();
        let (findings, complete) =
            assess_with_evidence(&snapshot, &[], Uuid::now_v7(), Some(evidence));
        assert!(!complete);
        assert!(
            findings
                .iter()
                .any(|f| f.field_path.as_deref() == Some("tags.graph.authority"))
        );
        snapshot.records.clear();
        let (findings, complete) = assess_with_evidence(&snapshot, &[], provider, Some(evidence));
        assert!(!complete);
        assert!(
            findings
                .iter()
                .any(|f| f.field_path.as_deref() == Some("tags.graph.authority"))
        );
    }
}
