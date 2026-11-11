//! NetBox read-only `InventorySource` adapter (PIP-86).
//!
//! Reads only. It lists each approved canonical resource page by page through
//! the NetBox repository and turns every record into a provider-neutral
//! [`SourceRecord`]. It never issues a POST/PATCH/DELETE, never bootstraps a
//! webhook and never mutates the provider.
//!
//! The approved entity list is supplied by the caller (the local inventory
//! catalog); the adapter refuses to start when the REST collection an approved
//! entity is bound to is not present in the provider's discovered registry, so a
//! resource is never silently dropped. The binding itself is typed: most keys
//! name their own collection, while `dcim.port-template-mappings` is projected
//! from the `rear_ports` items embedded in `dcim.front-port-templates`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::{
    InventorySource, SourceEntity, SourceIdentity, SourcePage, SourceRecord,
};
use crate::errors::AppError;
use crate::integrations::netbox::{NetboxPage, NetboxRepository, NetboxResourceRegistry};

use super::netbox_dataflow_source::{DataflowCapability, DataflowCollection, project_page};
use super::netbox_source_projection::{SourceBinding, page_cursor, project_port_template_mappings};

pub struct NetboxInventorySource {
    repository: NetboxRepository,
    provider_id: Uuid,
    /// The mandatory core manifest. Optional plugin entities are added by
    /// [`NetboxInventorySource::with_dataflow_capability`], never here.
    approved: Vec<SourceEntity>,
    dataflow: DataflowCapability,
    page_size: usize,
    generation_binding:
        Option<crate::contexts::sync::domain::inventory_sync::ConfiguredSourceEvidence>,
    /// One pull reads one snapshot. A projected resource must see exactly the
    /// parent page its direct sibling saw, so each collection page is fetched
    /// once and shared instead of being read twice over the wire.
    pages: Mutex<BTreeMap<(String, usize), Arc<NetboxPage>>>,
}

impl NetboxInventorySource {
    /// Build the source for an explicit, approved set of canonical entities.
    ///
    /// Every approved entity must be bound to a REST collection present in the
    /// provider registry; a missing collection fails construction with the exact
    /// key instead of being skipped. A derived entity additionally requires its
    /// canonical parent in the same approved manifest, so its relationship rows
    /// can resolve their endpoints.
    pub fn new(
        repository: NetboxRepository,
        registry: &NetboxResourceRegistry,
        provider_id: Uuid,
        approved: Vec<SourceEntity>,
        page_size: usize,
    ) -> Result<Self, AppError> {
        for entity in &approved {
            let binding = SourceBinding::of(entity);
            let collection = binding.collection(entity);
            if !registry.has_resource(collection) {
                return Err(AppError::Validation(format!(
                    "approved source entity '{}' is bound to collection '{collection}', which is not exposed by the provider registry",
                    entity.as_str()
                )));
            }
            if collection != entity.as_str()
                && !approved.iter().any(|parent| parent.as_str() == collection)
            {
                return Err(AppError::Validation(format!(
                    "derived source entity '{}' requires its canonical parent '{collection}' in the approved manifest",
                    entity.as_str()
                )));
            }
        }
        Ok(Self {
            repository,
            provider_id,
            approved,
            dataflow: DataflowCapability::Absent,
            page_size: page_size.max(1),
            generation_binding: None,
            pages: Mutex::new(BTreeMap::new()),
        })
    }

    /// Declare the resolved optional `data-flows` capability of this provider.
    ///
    /// `Present` adds the five plugin collections to the traversed manifest;
    /// every other state withholds them, so the pull neither requests them nor
    /// reports them as absent. A partial or unsupported plugin never reaches
    /// this call: resolution fails first with the exact field.
    pub(crate) fn with_dataflow_capability(
        mut self,
        registry: &NetboxResourceRegistry,
        capability: DataflowCapability,
    ) -> Result<Self, AppError> {
        if capability.is_present() {
            for collection in DataflowCollection::ALL {
                let entity = collection.entity()?;
                if self.approved.contains(&entity) {
                    return Err(AppError::Validation(format!(
                        "approved source entity '{}' is both core and optional",
                        entity.as_str()
                    )));
                }
                let binding = SourceBinding::of(&entity);
                if !registry.has_resource(binding.collection(&entity)) {
                    return Err(AppError::Validation(format!(
                        "declared data-flows entity '{}' is bound to collection '{}', which is not exposed by the provider registry",
                        entity.as_str(),
                        binding.collection(&entity)
                    )));
                }
            }
        }
        self.dataflow = capability;
        Ok(self)
    }

    /// Optional plugin entities of this provider, whether traversed or withheld.
    fn dataflow_entities(&self) -> Vec<SourceEntity> {
        DataflowCollection::ALL
            .into_iter()
            .filter_map(|collection| collection.entity().ok())
            .collect()
    }

    pub(crate) fn bind_generation(
        mut self,
        binding: crate::contexts::sync::domain::absence::ProviderGenerationBinding,
    ) -> Result<Self, AppError> {
        if binding.provider_id != self.provider_id {
            return Err(AppError::Validation(
                "sync.source_generation_provider_mismatch".into(),
            ));
        }
        self.generation_binding = Some(
            crate::contexts::sync::domain::inventory_sync::ConfiguredSourceEvidence::from_binding(
                binding,
            ),
        );
        Ok(self)
    }

    async fn list_page(
        &self,
        collection: &str,
        offset: usize,
    ) -> Result<Arc<NetboxPage>, AppError> {
        let key = (collection.to_string(), offset);
        if let Some(page) = self.pages.lock().await.get(&key) {
            return Ok(Arc::clone(page));
        }
        let page = Arc::new(
            self.repository
                .list(
                    collection,
                    Some(self.page_size),
                    Some(offset),
                    Some(HashMap::new()),
                    Some("id".to_string()),
                )
                .await?,
        );
        let mut pages = self.pages.lock().await;
        if pages
            .get(&(collection.to_string(), 0))
            .is_some_and(|first| first.count != page.count)
        {
            return Err(AppError::Validation(
                "source collection count changed during pagination".into(),
            ));
        }
        pages.insert(key, Arc::clone(&page));
        Ok(page)
    }
}

impl crate::contexts::sync::domain::inventory_sync::inventory_source_seal::Sealed
    for NetboxInventorySource
{
}

impl InventorySource for NetboxInventorySource {
    fn supported_entities(&self) -> Vec<SourceEntity> {
        let mut entities = self.approved.clone();
        if self.dataflow.is_present() {
            entities.extend(self.dataflow_entities());
        }
        entities
    }

    fn source_provider_id(&self) -> Option<Uuid> {
        Some(self.provider_id)
    }

    fn withheld_entities(&self) -> Vec<SourceEntity> {
        if self.dataflow.is_present() {
            Vec::new()
        } else {
            self.dataflow_entities()
        }
    }

    fn capability_token(&self) -> Option<&'static str> {
        Some(self.dataflow.as_str())
    }

    fn generation_binding(
        &self,
    ) -> Option<crate::contexts::sync::domain::inventory_sync::ConfiguredSourceEvidence> {
        self.generation_binding.clone()
    }

    async fn fetch_page(
        &self,
        entity: &SourceEntity,
        cursor: Option<&str>,
    ) -> Result<SourcePage, AppError> {
        if !self.approved.contains(entity)
            && !(self.dataflow.is_present() && DataflowCollection::of(entity).is_some())
        {
            return Err(AppError::Validation(
                "Source entity is outside the approved inventory manifest".into(),
            ));
        }
        let offset = match cursor {
            None => 0usize,
            Some(raw) => raw
                .parse::<usize>()
                .map_err(|_| AppError::Validation(format!("invalid source cursor '{raw}'")))?,
        };

        let binding = SourceBinding::of(entity);
        let page = self.list_page(binding.collection(entity), offset).await?;
        // Pagination is a property of the raw parent collection: one parent
        // record may yield several projected rows.
        let consumed = page.results.len();
        let next_cursor = page_cursor(offset, consumed, page.count)?;

        let records = if DataflowCollection::of(entity).is_some() {
            project_page(self.provider_id, entity, &page.results)?
        } else {
            match binding {
                SourceBinding::Collection => {
                    let mut records = Vec::with_capacity(consumed);
                    for value in page.results.iter().cloned() {
                        records.push(record_from_value(self.provider_id, entity, value)?);
                    }
                    records
                }
                SourceBinding::FrontPortTemplateRearPorts => {
                    project_port_template_mappings(self.provider_id, &page.results, entity)?
                }
            }
        };

        Ok(SourcePage {
            records,
            next_cursor,
        })
    }
}

/// Shared NetBox JSON-to-record boundary for HTTP reads and offline snapshots.
pub(crate) fn record_from_value(
    provider_id: Uuid,
    entity: &SourceEntity,
    value: Value,
) -> Result<SourceRecord, AppError> {
    let fields = value
        .as_object()
        .ok_or_else(|| AppError::Validation("netbox_capture:record_shape".into()))?;
    let external_id = fields
        .get("id")
        .ok_or_else(|| AppError::Validation("netbox_capture:record_identity".into()))
        .and_then(normalize_netbox_id)?;
    Ok(SourceRecord {
        identity: SourceIdentity::new(provider_id, entity.clone(), external_id)?,
        fields: fields.clone(),
    })
}
