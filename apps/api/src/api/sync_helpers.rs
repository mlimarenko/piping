use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::app::AppState;
use crate::config::NetboxConfig;
use crate::config::constants::SYNC_LOCK_TTL_SECONDS;
use crate::contexts::sync::application::{
    InventoryPullRunner, InventoryPullService, SourceService,
};
use crate::contexts::sync::domain::{
    LocalInventoryCatalogView, Provider, ProviderType, parse_custom_provider_config,
};
use crate::contexts::sync::infrastructure::persistence::local_inventory_target::LocalInventoryCatalog;
use crate::contexts::sync::infrastructure::persistence::{
    SqlxOutboundEventRepository, SqlxProviderRepository, SqlxSourceRuntimeRepository,
    repositories::repository::ProviderRepository,
};
use crate::contexts::sync::infrastructure::providers::NetboxInventorySource;
use crate::errors::AppError;
use crate::integrations::netbox::{
    NetboxClient, NetboxRepository, NetboxResourceRegistry, NetboxSchemaProvider,
};

pub fn build_source_service(
    state: &AppState,
) -> Arc<
    SourceService<
        SqlxProviderRepository,
        SqlxSourceRuntimeRepository,
        SqlxOutboundEventRepository,
        crate::contexts::secrets::infrastructure::persistence::SqlxSecretRepository,
        crate::contexts::secrets::infrastructure::persistence::DbSecretStore,
    >,
> {
    state.services.source_service.clone()
}

pub async fn build_netbox_registry_for_provider(
    state: &AppState,
    provider: &Provider,
) -> Result<Arc<NetboxResourceRegistry>, AppError> {
    let config = state
        .services
        .source_service
        .resolve_netbox_config_for_provider(provider)
        .await?;
    build_netbox_registry(state, config).await
}

/// Dispatch to the correct registry builder based on provider type.
///
/// - NetBox: fetches and parses the live OpenAPI schema from the NetBox instance.
/// - Custom: returns an empty registry; resource discovery is not yet supported.
pub async fn build_registry_for_provider(
    state: &AppState,
    provider: &Provider,
) -> Result<Arc<NetboxResourceRegistry>, AppError> {
    match provider.provider_type {
        ProviderType::Netbox => build_netbox_registry_for_provider(state, provider).await,
        ProviderType::Custom => {
            let _ = parse_custom_provider_config(&provider.config).map_err(AppError::Validation)?;
            Ok(Arc::new(NetboxResourceRegistry::empty()))
        }
    }
}

pub async fn refresh_netbox_registry_for_provider(
    state: &AppState,
    provider: &Provider,
) -> Result<Arc<NetboxResourceRegistry>, AppError> {
    let config = state
        .services
        .source_service
        .resolve_netbox_config_for_provider(provider)
        .await?;
    let cache = Arc::new(Mutex::new(state.redis.clone()));
    let schema_provider = NetboxSchemaProvider::new_with_cache(config, cache);
    let schema = schema_provider.refresh_schema().await?;
    Ok(Arc::new(NetboxResourceRegistry::from_schema(schema)?))
}

pub async fn invalidate_netbox_registry_cache_for_provider(
    state: &AppState,
    provider: &Provider,
) -> Result<(), AppError> {
    let config = state
        .services
        .source_service
        .resolve_netbox_config_for_provider(provider)
        .await?;
    let cache = Arc::new(Mutex::new(state.redis.clone()));
    let schema_provider = NetboxSchemaProvider::new_with_cache(config, cache);
    schema_provider.invalidate_cache().await
}

async fn build_netbox_registry(
    state: &AppState,
    netbox_config: NetboxConfig,
) -> Result<Arc<NetboxResourceRegistry>, AppError> {
    let cache = Arc::new(Mutex::new(state.redis.clone()));
    let schema_provider = NetboxSchemaProvider::new_with_cache(netbox_config, cache);
    let schema = schema_provider.get_schema().await?;
    Ok(Arc::new(NetboxResourceRegistry::from_schema(schema)?))
}

/// Build the object-safe pull runner for one provider.
///
pub async fn build_inventory_pull_runner(
    state: &AppState,
    provider: &Provider,
) -> Result<Arc<dyn InventoryPullRunner>, AppError> {
    if provider.provider_type != ProviderType::Netbox {
        return Err(AppError::Validation(
            "Local inventory pull requires a NetBox source".into(),
        ));
    }
    let config = state
        .services
        .source_service
        .resolve_netbox_config_for_provider(provider)
        .await?;
    let registry = build_netbox_registry(state, config.clone()).await?;
    let repository = NetboxRepository::new(NetboxClient::from_config(&config)?, registry.clone());
    let target = LocalInventoryCatalog::new(state.db_pool.clone());
    let approved = target
        .resources()
        .into_iter()
        .map(|resource| resource.source_entity)
        .collect();
    let source = NetboxInventorySource::new(
        repository,
        &registry,
        provider.id,
        approved,
        config.page_size,
    )?;
    Ok(Arc::new(InventoryPullService::new(
        state.repositories.sync_mapping_configurations.clone(),
        state.repositories.sync_jobs.clone(),
        source,
        target,
        state.redis.clone(),
        SYNC_LOCK_TTL_SECONDS as usize,
    )))
}

/// The local inventory catalogue served by the target-resource routes.
///
pub fn local_inventory_catalog(
    state: &AppState,
) -> Result<Arc<dyn LocalInventoryCatalogView>, AppError> {
    Ok(Arc::new(LocalInventoryCatalog::new(state.db_pool.clone())))
}

/// Resolve default provider for a resource. Returns provider_id of first enabled provider.
/// resource_key is reserved for future routing.
pub async fn resolve_netbox_provider_for_resource(
    state: &AppState,
    _resource_key: &str,
) -> Result<Option<Uuid>, AppError> {
    let providers = state.repositories.sync_providers.list().await?;
    let provider = providers
        .iter()
        .find(|p| {
            p.provider_type == ProviderType::Netbox
                && p.is_enabled
                && p.config
                    .as_object()
                    .and_then(|map| map.get("sync_mode"))
                    .and_then(Value::as_str)
                    .map(|mode| mode == "bidirectional")
                    .unwrap_or(false)
        })
        .cloned()
        .or_else(|| {
            providers
                .into_iter()
                .find(|p| p.provider_type == ProviderType::Netbox && p.is_enabled)
        });
    Ok(provider.map(|p| p.id))
}
