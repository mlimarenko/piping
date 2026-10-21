use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::contexts::inventory::application::config_resolver::{
    compute_secret_presence, merge_value, redact_secret_fields, resolve_config_contexts_ordered,
};
use crate::contexts::inventory::domain::{ConfigContext, ConfigContextBinding};
use crate::contexts::inventory::infrastructure::ConfigContextRepository;
use crate::contexts::inventory::infrastructure::persistence::repositories::ConfigContextBindingListFilters;
use crate::contexts::secrets::infrastructure::{SecretRepository, SecretScopeSecrets};
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;

/// One config context that contributed to a rendered config context, in
/// resolution order (lowest precedence first, highest precedence last).
#[derive(Debug, Clone)]
pub struct RenderedConfigContextSource {
    pub context_id: Uuid,
    pub context_name: String,
    pub weight: i32,
}

/// Result of rendering an entity's config context: the merged inherited
/// configuration with the entity's `local_context_data` applied as the
/// highest-priority layer, plus the resolution chain. Connection secrets are
/// redacted from `rendered` so the read path never reveals credentials.
#[derive(Debug, Clone)]
pub struct RenderedConfigContext {
    pub rendered: Value,
    pub local_context_data: Option<Value>,
    pub sources: Vec<RenderedConfigContextSource>,
}

/// Request for one page of config contexts applicable to a resolved scope.
///
/// The scope (type, id and whether the hierarchy is expanded) and the
/// post-expansion name/`is_active` filters travel together because they are
/// resolved in one pass; grouping them keeps the service method within the
/// `clippy::too_many_arguments` bound without dropping any parameter.
pub struct ConfigContextScopePageRequest<'a> {
    pub scope_type: &'a str,
    pub scope_id: Uuid,
    pub include_hierarchy: bool,
    pub name: Option<&'a str>,
    pub is_active: &'a [String],
    pub limit: u32,
    pub offset: u32,
}

pub struct ConfigQueryService<CRepo, SRepo>
where
    CRepo: ConfigContextRepository,
    SRepo: SecretRepository,
{
    config_contexts: Arc<CRepo>,
    secrets: Arc<SRepo>,
}

impl<CRepo, SRepo> ConfigQueryService<CRepo, SRepo>
where
    CRepo: ConfigContextRepository,
    SRepo: SecretRepository,
{
    pub fn new(config_contexts: Arc<CRepo>, secrets: Arc<SRepo>) -> Self {
        Self {
            config_contexts,
            secrets,
        }
    }

    pub async fn get_context(&self, context_id: Uuid) -> Result<Option<ConfigContext>, AppError> {
        self.config_contexts.get_by_id(context_id).await
    }

    pub async fn list_contexts_page(
        &self,
        name: Option<&str>,
        is_active: &[String],
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContext>, AppError> {
        self.config_contexts
            .list_contexts_page(name, is_active, sort, limit, offset)
            .await
    }

    /// Full unpaginated list for internal configuration resolvers that need
    /// every matching context. HTTP list endpoints must use `list_contexts_page`.
    pub async fn list_all_contexts(
        &self,
        name: Option<&str>,
        is_active: &[String],
    ) -> Result<Vec<ConfigContext>, AppError> {
        self.config_contexts
            .list_all_contexts(name, is_active)
            .await
    }

    pub async fn create_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError> {
        self.config_contexts.create_context(context).await
    }

    pub async fn update_context(&self, context: ConfigContext) -> Result<ConfigContext, AppError> {
        self.config_contexts.update_context(context).await
    }

    pub async fn delete_context(&self, context_id: Uuid) -> Result<bool, AppError> {
        self.config_contexts.delete_context(context_id).await
    }

    /// Resolves the full (unpaginated) list of config contexts applicable to
    /// a given scope. Used by internal resolvers that need every context to
    /// compute merged configuration.
    pub async fn resolve_all_contexts_for_scope(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        include_hierarchy: bool,
    ) -> Result<Vec<ConfigContext>, AppError> {
        if include_hierarchy {
            self.config_contexts
                .list_for_scope_hierarchy(scope_type, scope_id)
                .await
        } else {
            self.config_contexts
                .list_for_scope(scope_type, scope_id)
                .await
        }
    }

    /// Render the effective config context for an entity scope (NetBox
    /// "rendered config context").
    ///
    /// Resolves every config context bound to the entity's scope ancestry
    /// (`scope_type` is `"device"` or `"vm"`), merges them in precedence order
    /// (weight, then scope precedence, then UUIDv7 — the SQL already returns
    /// them sorted), then merges the entity's `local_context_data` on top as the
    /// highest-priority layer so it overrides all inherited contexts.
    ///
    /// Connection credentials that may live in context `data`
    /// (`piping.connections.*.credentials.*`) are redacted from the rendered
    /// output, so this read path never reveals secrets.
    pub async fn render_config_context(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        local_context_data: Option<Value>,
    ) -> Result<RenderedConfigContext, AppError> {
        let contexts = self
            .resolve_all_contexts_for_scope(scope_type, scope_id, true)
            .await?;

        let sources = contexts
            .iter()
            .map(|context| RenderedConfigContextSource {
                context_id: context.id,
                context_name: context.name.clone(),
                weight: context.weight,
            })
            .collect();

        let mut rendered = resolve_config_contexts_ordered(&contexts);
        if let Some(local) = local_context_data.as_ref() {
            merge_value(&mut rendered, local);
        }

        // Strip connection credentials before returning. An empty attribution
        // map is sufficient: secret paths are derived from the config shape, not
        // from attribution (which only supplies a source label we do not use).
        let secret_presence = compute_secret_presence(&rendered, &HashMap::new());
        redact_secret_fields(&mut rendered, &secret_presence);

        Ok(RenderedConfigContext {
            rendered,
            local_context_data,
            sources,
        })
    }

    /// Paginated list of config contexts applicable to a given scope, with
    /// optional name/`is_active` filters applied after hierarchy expansion
    /// (the hierarchy resolution itself is not expressible as a single SQL
    /// and must be computed server-side).
    pub async fn list_contexts_for_scope_page(
        &self,
        request: ConfigContextScopePageRequest<'_>,
    ) -> Result<PageResult<ConfigContext>, AppError> {
        let ConfigContextScopePageRequest {
            scope_type,
            scope_id,
            include_hierarchy,
            name,
            is_active,
            limit,
            offset,
        } = request;
        let mut contexts = self
            .resolve_all_contexts_for_scope(scope_type, scope_id, include_hierarchy)
            .await?;
        if let Some(needle) = name.map(str::to_ascii_lowercase) {
            contexts.retain(|ctx| ctx.name.to_ascii_lowercase().contains(needle.as_str()));
        }
        if !is_active.is_empty() {
            contexts.retain(|ctx| {
                is_active.iter().any(|value| {
                    (value == "true" && ctx.is_active) || (value == "false" && !ctx.is_active)
                })
            });
        }

        let total = contexts.len() as u64;
        let items: Vec<ConfigContext> = contexts
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        Ok(PageResult {
            items,
            total,
            limit,
            offset,
        })
    }

    pub async fn list_context_bindings(
        &self,
        context_id: Uuid,
    ) -> Result<Vec<ConfigContextBinding>, AppError> {
        self.config_contexts.list_bindings(context_id).await
    }

    pub async fn list_context_bindings_page(
        &self,
        context_id: Uuid,
        filters: &ConfigContextBindingListFilters,
        sort: Option<&SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<ConfigContextBinding>, AppError> {
        self.config_contexts
            .list_bindings_page(context_id, filters, sort, limit, offset)
            .await
    }

    pub async fn create_context_binding(
        &self,
        binding: ConfigContextBinding,
    ) -> Result<ConfigContextBinding, AppError> {
        self.config_contexts.create_binding(binding).await
    }

    pub async fn delete_context_binding(&self, binding_id: Uuid) -> Result<bool, AppError> {
        self.config_contexts.delete_binding(binding_id).await
    }

    pub async fn list_scope_secrets(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<SecretScopeSecrets, AppError> {
        self.secrets.list_scope_secrets(scope_type, scope_id).await
    }
}
