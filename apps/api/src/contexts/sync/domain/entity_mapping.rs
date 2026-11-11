// Source-entity to target-entity link inside a
// MappingConfiguration version.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_sync::MappingConfigDocument;
use super::inventory_target_schema::InventoryEntity;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMapping {
    pub id: Uuid,
    pub mapping_config_logical_id: Uuid,
    pub mapping_config_version_id: i32,
    /// E.g. `netbox.dcim.device`.
    pub source_entity: String,
    /// E.g. `piping.inventory.device`.
    pub target_entity: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// Normalize one NetBox model segment: `site-groups` and `site_groups` are the
/// same model, and NetBox event rules expose it case-insensitively.
fn normalize_model(segment: &str) -> String {
    segment
        .chars()
        .filter(|character| *character != '-' && *character != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Singular form of a NetBox resource segment: `sites`→`site`,
/// `ip-addresses`→`ipaddress`, `prefixes`→`prefix`.
fn singular_model(resource: &str) -> &str {
    if ["ses", "xes", "zes", "ches", "shes"]
        .iter()
        .any(|suffix| resource.ends_with(suffix))
    {
        return &resource[..resource.len() - 2];
    }
    resource.strip_suffix('s').unwrap_or(resource)
}

/// Whether a NetBox event model (`dcim.site`) names the resource of a mapping
/// source entity (`dcim.sites` or `netbox.dcim.site`).
///
/// A mapping may store the resource singular or plural; NetBox always exposes
/// the model singular, so both spellings match one entity.
#[must_use]
pub fn model_matches_source_entity(source_entity: &str, model: &str) -> bool {
    let entity = source_entity.strip_prefix("netbox.").unwrap_or(source_entity);
    let (Some((entity_app, entity_resource)), Some((model_app, model_resource))) =
        (entity.split_once('.'), model.trim().split_once('.'))
    else {
        return false;
    };
    if !entity_app.eq_ignore_ascii_case(model_app) {
        return false;
    }
    let resource = normalize_model(entity_resource);
    let model = normalize_model(model_resource);
    model == resource || model == singular_model(&resource)
}

/// The enabled entity mapping that carries a live event for `model`.
///
/// The mapping must cover the closed inventory entity exactly: its stored
/// source and target keys are the ones the batch writer resolves.
#[must_use]
pub fn live_entity_mapping<'a>(
    config: &'a MappingConfigDocument,
    model: &str,
) -> Option<(InventoryEntity, &'a EntityMapping)> {
    let entity = InventoryEntity::all()
        .find(|entity| model_matches_source_entity(entity.source_key(), model))?;
    config
        .entity_mappings
        .iter()
        .find(|mapping| {
            mapping.enabled
                && model_matches_source_entity(&mapping.source_entity, model)
                && mapping.target_entity == entity.target_key()
        })
        .map(|mapping| (entity, mapping))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_models_match_singular_plural_and_prefixed_source_entities() {
        for (source_entity, model) in [
            ("dcim.sites", "dcim.site"),
            ("netbox.dcim.site", "dcim.site"),
            ("dcim.site-groups", "dcim.sitegroup"),
            ("tenancy.tenants", "tenancy.tenant"),
            ("ipam.ip-addresses", "ipam.ipaddress"),
            ("virtualization.virtual-machines", "virtualization.virtualmachine"),
        ] {
            assert!(
                model_matches_source_entity(source_entity, model),
                "{source_entity} for {model}"
            );
        }
    }

    #[test]
    fn mismatched_app_or_resource_does_not_match() {
        for (source_entity, model) in [
            ("dcim.sites", "tenancy.tenant"),
            ("dcim.sites", "dcim.sitegroups"),
            ("dcim.sites", "site"),
            ("dcim.sites", ""),
            ("not-an-entity", "dcim.site"),
        ] {
            assert!(
                !model_matches_source_entity(source_entity, model),
                "{source_entity} for {model}"
            );
        }
    }

    #[test]
    fn live_mapping_accepts_prefixed_singular_source_entity() {
        use crate::contexts::sync::domain::{
            ConflictMode, MappingConfigurationStatus, MappingDirection, MappingRef, MappingTarget,
        };
        let logical_id = Uuid::now_v7();
        let document = MappingConfigDocument {
            absence_policy: Default::default(),
            reference: MappingRef::new(logical_id, 1),
            name: "live".into(),
            description: None,
            source_provider_id: Uuid::now_v7(),
            target: MappingTarget::LocalInventory,
            status: MappingConfigurationStatus::Active,
            direction: MappingDirection::Bidirectional,
            conflict_mode: ConflictMode::NetboxWins,
            created_by: None,
            created_at: Utc::now(),
            retired_at: None,
            entity_mappings: vec![EntityMapping {
                id: Uuid::now_v7(),
                mapping_config_logical_id: logical_id,
                mapping_config_version_id: 1,
                source_entity: "netbox.dcim.site".into(),
                target_entity: InventoryEntity::Site.target_key(),
                enabled: true,
                created_at: Utc::now(),
            }],
            field_mappings: Vec::new(),
        };
        let (mapped, _) = live_entity_mapping(&document, "dcim.site").expect("live mapping");
        assert_eq!(mapped, InventoryEntity::Site);
    }
}
