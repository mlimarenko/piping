use std::sync::Arc;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::app::AppState;
use crate::config::NetboxConfig;
use crate::config::constants::SYNC_LOCK_TTL_SECONDS;
use crate::contexts::sync::application::{
    InventoryPullRunner, InventoryPullService, PolicyRequestCommitAuthority, SourceService,
    netbox_provider_config::ProviderGenerationBinding,
};
use crate::contexts::sync::domain::{
    LocalInventoryCatalogView, Provider, ProviderType, parse_custom_provider_config,
};
use crate::contexts::sync::infrastructure::persistence::local_inventory_target::LocalInventoryCatalog;
use crate::contexts::sync::infrastructure::persistence::{
    SqlxMappingConfigurationRepository, SqlxOutboundEventRepository, SqlxProviderRepository,
    SqlxSourceRuntimeRepository, repositories::repository::ProviderRepository,
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
        SqlxMappingConfigurationRepository,
        crate::contexts::secrets::infrastructure::persistence::SqlxSecretRepository,
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
    let resolved = state
        .services
        .source_service
        .resolve_netbox_source_for_provider(provider)
        .await?;
    let binding = if provider.is_enabled && resolved.credential.is_some() {
        Some(ProviderGenerationBinding::capture(provider, &resolved)?)
    } else {
        None
    };
    let config = resolved.config;
    let registry = build_netbox_registry(state, config.clone()).await?;
    // Optional plugin capability, resolved from the same canonical registry the
    // core manifest is validated against. An absent plugin contributes no
    // namespace; a declared plugin with an unsupported shape fails here with
    // the exact field instead of silently shrinking the pull.
    let dataflow = crate::contexts::sync::infrastructure::providers::netbox_dataflow_source::resolve_capability(&registry)?;
    let repository = NetboxRepository::new(NetboxClient::from_config(&config)?, registry.clone());
    let target = LocalInventoryCatalog::new(state.db_pool.clone())
        .with_generation_authority(state.services.source_service.clone());
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
    )?
    .with_dataflow_capability(&registry, dataflow)?;
    let source = match binding {
        Some(binding) => source.bind_generation(binding)?,
        None => source,
    };
    Ok(Arc::new(InventoryPullService::new(
        state.repositories.sync_mapping_configurations.clone(),
        state.repositories.sync_jobs.clone(),
        source,
        target,
        state.redis.clone(),
        SYNC_LOCK_TTL_SECONDS as usize,
        Arc::new(PolicyRequestCommitAuthority::new(
            state.services.policy_evaluation.clone(),
            state.services.user_api_tokens.clone(),
            Arc::new(state.repositories.users.clone()),
        )),
    )))
}

pub fn build_pull_schedule_service(
    state: &AppState,
) -> crate::contexts::sync::application::pull_schedule::PullScheduleService {
    use crate::contexts::sync::application::pull_schedule::PullScheduleService;
    use crate::contexts::sync::infrastructure::persistence::SqlxPullScheduleRepository;
    PullScheduleService::new(
        SqlxPullScheduleRepository::new(state.db_pool.clone()),
        state.repositories.sync_providers.clone(),
        state.repositories.sync_mapping_configurations.clone(),
        Arc::new(PolicyRequestCommitAuthority::new(
            state.services.policy_evaluation.clone(),
            state.services.user_api_tokens.clone(),
            Arc::new(state.repositories.users.clone()),
        )),
    )
}

/// The local inventory catalogue served by the target-resource routes.
///
pub fn local_inventory_catalog(
    state: &AppState,
) -> Result<Arc<dyn LocalInventoryCatalogView>, AppError> {
    Ok(Arc::new(LocalInventoryCatalog::new(state.db_pool.clone())))
}

/// Resolve and capture routing before local persistence. A bound row never changes source.
pub async fn resolve_inventory_outbound_destination(
    state: &AppState,
    entity: crate::contexts::sync::domain::InventoryEntity,
    local_id: Uuid,
    create: bool,
) -> Result<Option<Uuid>, AppError> {
    let catalog = LocalInventoryCatalog::new(state.db_pool.clone());
    if let Some(binding) = catalog.binding_for_local(entity, local_id).await? {
        return Ok(Some(binding.identity.provider_id));
    }
    if !create {
        return Ok(None);
    }
    let providers = state.repositories.sync_providers.list().await?;
    let mut eligible = providers.iter().filter(|provider| {
        provider.provider_type == ProviderType::Netbox
            && provider.is_enabled
            && provider.allows_remote_writes()
    });
    let destination = eligible.next().map(|provider| provider.id);
    if eligible.next().is_some() {
        return Err(AppError::Conflict(
            "Multiple writable NetBox destinations; select a single source before creating a Site"
                .into(),
        ));
    }
    Ok(destination)
}
