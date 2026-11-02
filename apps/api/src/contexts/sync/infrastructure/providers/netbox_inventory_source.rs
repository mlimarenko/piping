//! NetBox read-only `InventorySource` adapter (PIP-86).
//!
//! Reads only. It lists each approved canonical resource page by page through
//! the NetBox repository and turns every record into a provider-neutral
//! [`SourceRecord`]. It never issues a POST/PATCH/DELETE, never bootstraps a
//! webhook and never mutates the provider.
//!
//! The approved entity list is supplied by the caller (the local inventory
//! catalog); the adapter refuses to start when an approved entity is not
//! present in the provider's discovered registry, so a resource is never
//! silently dropped.

use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use crate::contexts::sync::domain::inventory_sync::normalize_netbox_id;
use crate::contexts::sync::domain::{
    InventorySource, SourceEntity, SourceIdentity, SourcePage, SourceRecord,
};
use crate::errors::AppError;
use crate::integrations::netbox::{NetboxRepository, NetboxResourceRegistry};

pub struct NetboxInventorySource {
    repository: NetboxRepository,
    provider_id: Uuid,
    approved: Vec<SourceEntity>,
    page_size: usize,
}

impl NetboxInventorySource {
    /// Build the source for an explicit, approved set of canonical entities.
    ///
    /// Every approved entity must exist in the provider registry; a missing
    /// one fails construction with the exact key instead of being skipped.
    pub fn new(
        repository: NetboxRepository,
        registry: &NetboxResourceRegistry,
        provider_id: Uuid,
        approved: Vec<SourceEntity>,
        page_size: usize,
    ) -> Result<Self, AppError> {
        for entity in &approved {
            if !registry.has_resource(entity.as_str()) {
                return Err(AppError::Validation(format!(
                    "approved source entity '{}' is not exposed by the provider registry",
                    entity.as_str()
                )));
            }
        }
        Ok(Self {
            repository,
            provider_id,
            approved,
            page_size: page_size.max(1),
        })
    }
}

impl InventorySource for NetboxInventorySource {
    fn supported_entities(&self) -> Vec<SourceEntity> {
        self.approved.clone()
    }

    async fn fetch_page(
        &self,
        entity: &SourceEntity,
        cursor: Option<&str>,
    ) -> Result<SourcePage, AppError> {
        if !self.approved.contains(entity) {
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

        let page = self
            .repository
            .list(
                entity.as_str(),
                Some(self.page_size),
                Some(offset),
                Some(HashMap::new()),
                Some("id".to_string()),
            )
            .await?;

        let mut records = Vec::with_capacity(page.results.len());
        for value in page.results {
            records.push(record_from_value(self.provider_id, entity, value)?);
        }

        let fetched = offset
            .checked_add(records.len())
            .ok_or_else(|| AppError::Validation("Source page offset overflow".into()))?;
        if fetched > page.count || (records.is_empty() && fetched < page.count) {
            return Err(AppError::Validation(
                "Source pagination contradicts its declared record count".into(),
            ));
        }
        let next_cursor = if fetched == page.count {
            None
        } else {
            Some(fetched.to_string())
        };

        Ok(SourcePage {
            records,
            next_cursor,
        })
    }
}

/// Shared NetBox JSON-to-record boundary for HTTP reads and offline snapshots.
pub(super) fn record_from_value(
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
