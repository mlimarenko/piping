use redis::aio::ConnectionManager;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::inventory::application::config_resolver::merge_value;
use crate::contexts::inventory::application::resolve_config_contexts_ordered;
use crate::contexts::inventory::infrastructure::caching::HostConfigCache;
use crate::contexts::inventory::infrastructure::persistence::ConfigContextRepository;
use crate::contexts::secrets::application::secret_resolver::SecretResolutionContext;
use crate::contexts::secrets::application::{
    SecretStore, SecretVisibilityChecker, resolve_secret_visibility, resolve_secrets,
};
use crate::contexts::secrets::domain::{SecretBinding, SecretVariable};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct MaterializedTargetConfig {
    pub config: Value,
    pub secret_values: Value,
}

pub struct MaterializeTargetConfigInput<'a> {
    pub redis: Option<&'a mut ConnectionManager>,
    pub target_id: Uuid,
    pub local_context_data: Option<Value>,
    pub secret_variables: &'a [SecretVariable],
    pub secret_bindings: &'a [SecretBinding],
    pub secret_store: Option<&'a dyn SecretStore>,
    pub access_control: Option<&'a dyn SecretVisibilityChecker>,
    pub request_context: Option<&'a RequestContext>,
    pub audit: Option<&'a dyn AuditRecorder>,
    pub actor_id: Option<Uuid>,
}

pub struct MaterializationService<CRepo>
where
    CRepo: ConfigContextRepository,
{
    config_repository: CRepo,
    cache: HostConfigCache,
}

#[expect(
    async_fn_in_trait,
    reason = "native async trait methods are retained for this domain port; callers do not require an implementation-independent Send bound on the returned future"
)]
pub trait TargetMaterializer: Send + Sync {
    async fn materialize_target_config(
        &self,
        input: MaterializeTargetConfigInput<'_>,
    ) -> Result<MaterializedTargetConfig, AppError>;

    async fn invalidate_target_cache(
        &self,
        conn: &mut ConnectionManager,
        target_id: Uuid,
    ) -> Result<(), AppError>;

    /// Loads the authoritative per-launch `{{target.*}}` variable namespace for
    /// a connection target. The namespace is overlaid under the reserved
    /// `target` key during launch parameter resolution and is never persisted or
    /// merged into the cached host config.
    async fn load_target_variable_namespace(
        &self,
        target_id: Uuid,
    ) -> Result<Option<Value>, AppError>;
}

impl<CRepo> MaterializationService<CRepo>
where
    CRepo: ConfigContextRepository,
{
    pub fn new(config_repository: CRepo, cache: HostConfigCache) -> Self {
        Self {
            config_repository,
            cache,
        }
    }

    pub async fn materialize_target_config(
        &self,
        input: MaterializeTargetConfigInput<'_>,
    ) -> Result<MaterializedTargetConfig, AppError> {
        let MaterializeTargetConfigInput {
            mut redis,
            target_id,
            local_context_data,
            secret_variables,
            secret_bindings,
            secret_store,
            access_control,
            request_context,
            audit,
            actor_id,
        } = input;
        let local_context = local_context_data
            .and_then(select_local_context_data)
            .and_then(normalize_local_context_data);
        let can_view_secrets = resolve_secret_visibility(access_control, request_context).await?;
        let cache_context = build_cache_context(CacheContextInput {
            can_view_secrets,
            has_secret_store: secret_store.is_some(),
        });

        let mut cached_config = None;
        let use_cache = local_context.is_none();
        if let Some(conn) = redis.as_deref_mut()
            && use_cache
        {
            cached_config = self
                .cache
                .get_with_context(conn, target_id, &cache_context)
                .await?;
            crate::observability::set_attribute("cache.name", "connection_target_config");
            crate::observability::set_attribute("cache.context", &cache_context);
            crate::observability::set_attribute(
                "cache.hit",
                if cached_config.is_some() {
                    "true"
                } else {
                    "false"
                },
            );
        }

        let mut resolved_config = if let Some(cached) = cached_config {
            cached
        } else {
            let contexts = self
                .config_repository
                .list_for_scope_hierarchy("connection_target", target_id)
                .await?;

            resolve_config_contexts_ordered(&contexts)
        };
        if let Some(local_context) = local_context {
            merge_value(&mut resolved_config, &local_context);
        }

        let mut secret_values = resolve_secrets(
            secret_variables,
            secret_bindings,
            secret_store,
            SecretResolutionContext {
                access_control,
                request_context,
                audit,
                actor_id,
                can_view_override: Some(can_view_secrets),
            },
        )
        .await?;
        if let Some(config_secret_values) = resolve_config_secret_values(
            &resolved_config,
            secret_store,
            SecretResolutionContext {
                access_control,
                request_context,
                audit,
                actor_id,
                can_view_override: Some(can_view_secrets),
            },
        )
        .await?
        {
            merge_config_secret_values(&mut secret_values, config_secret_values);
        }
        let secrets_resolved =
            can_view_secrets && secret_store.is_some() && has_secret_entries(&secret_values);

        if let Some(conn) = redis
            && use_cache
            && !secrets_resolved
        {
            self.cache
                .set_with_context(conn, target_id, &cache_context, &resolved_config)
                .await?;
        }

        Ok(MaterializedTargetConfig {
            config: resolved_config,
            secret_values,
        })
    }

    pub async fn invalidate_target_cache(
        &self,
        conn: &mut ConnectionManager,
        target_id: Uuid,
    ) -> Result<(), AppError> {
        let _ = target_id;
        let _ = self.cache.bump_generation(conn).await?;
        Ok(())
    }

    pub async fn load_target_variable_namespace(
        &self,
        target_id: Uuid,
    ) -> Result<Option<Value>, AppError> {
        self.config_repository
            .load_target_variable_namespace(target_id)
            .await
    }
}

impl<CRepo> TargetMaterializer for MaterializationService<CRepo>
where
    CRepo: ConfigContextRepository,
{
    async fn materialize_target_config(
        &self,
        input: MaterializeTargetConfigInput<'_>,
    ) -> Result<MaterializedTargetConfig, AppError> {
        MaterializationService::materialize_target_config(self, input).await
    }

    async fn invalidate_target_cache(
        &self,
        conn: &mut ConnectionManager,
        target_id: Uuid,
    ) -> Result<(), AppError> {
        MaterializationService::invalidate_target_cache(self, conn, target_id).await
    }

    async fn load_target_variable_namespace(
        &self,
        target_id: Uuid,
    ) -> Result<Option<Value>, AppError> {
        MaterializationService::load_target_variable_namespace(self, target_id).await
    }
}

fn select_local_context_data(local_context_data: Value) -> Option<Value> {
    match local_context_data {
        Value::Null => None,
        Value::Object(map) => {
            let device_context = map
                .get("device")
                .cloned()
                .filter(|value| !value.is_null())
                .or_else(|| {
                    map.get("virtual_machine")
                        .cloned()
                        .filter(|value| !value.is_null())
                })
                .or_else(|| map.get("vm").cloned().filter(|value| !value.is_null()));
            Some(device_context.unwrap_or(Value::Object(map)))
        }
        other => Some(other),
    }
}

fn normalize_local_context_data(local_context_data: Value) -> Option<Value> {
    match local_context_data {
        Value::Null => None,
        Value::Object(map) => Some(Value::Object(map)),
        Value::Array(items) if items.is_empty() => None,
        other => Some(other),
    }
}

async fn resolve_config_secret_values(
    config: &Value,
    secret_store: Option<&dyn SecretStore>,
    resolution: SecretResolutionContext<'_>,
) -> Result<Option<Value>, AppError> {
    let SecretResolutionContext {
        access_control,
        request_context,
        audit,
        actor_id,
        can_view_override,
    } = resolution;
    let mut links = Map::new();
    collect_secret_links(config, "", &mut links);
    if links.is_empty() {
        return Ok(None);
    }

    let variables = links
        .iter()
        .filter_map(|(path, value)| match value {
            Value::String(secret_link) => {
                parse_secret_link_id(secret_link).map(|secret_id| SecretVariable {
                    id: secret_id,
                    key: path.clone(),
                    is_masked: false,
                    is_protected: false,
                    kind: crate::contexts::secrets::domain::SECRET_KIND_FIELD.to_string(),
                    description: None,
                    retire_with_field_usage: false,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    if variables.is_empty() {
        return Ok(None);
    }

    let resolved = resolve_secrets(
        &variables,
        &[],
        secret_store,
        SecretResolutionContext {
            access_control,
            request_context,
            audit,
            actor_id,
            can_view_override,
        },
    )
    .await?;

    match &resolved {
        Value::Object(map) if map.is_empty() => Ok(None),
        _ => Ok(Some(resolved)),
    }
}

fn merge_config_secret_values(secret_values: &mut Value, config_values: Value) {
    match secret_values {
        Value::Object(map) => {
            map.insert("config_context".to_string(), config_values);
        }
        _ => {
            let mut map = Map::new();
            map.insert("config_context".to_string(), config_values);
            *secret_values = Value::Object(map);
        }
    }
}

fn collect_secret_links(value: &Value, path: &str, links: &mut Map<String, Value>) {
    match value {
        Value::String(secret_link) => {
            if parse_secret_link_id(secret_link).is_some() {
                let key = if path.is_empty() {
                    "root".to_string()
                } else {
                    path.to_string()
                };
                links.insert(key, Value::String(secret_link.clone()));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let child_path = if path.is_empty() {
                    format!("[{}]", index)
                } else {
                    format!("{}[{}]", path, index)
                };
                collect_secret_links(item, &child_path, links);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                collect_secret_links(value, &child_path, links);
            }
        }
        _ => {}
    }
}

fn parse_secret_link_id(value: &str) -> Option<Uuid> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Uuid::parse_str(trimmed).ok()
}

#[derive(Debug, Clone, Copy)]
struct CacheContextInput {
    can_view_secrets: bool,
    has_secret_store: bool,
}

fn build_cache_context(input: CacheContextInput) -> String {
    let visibility = if input.can_view_secrets {
        "allow"
    } else {
        "deny"
    };
    let secret_source = if input.has_secret_store {
        "store"
    } else {
        "metadata"
    };
    format!("visibility:{visibility}:secrets:{secret_source}")
}

fn has_secret_entries(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Null => false,
        _ => true,
    }
}
