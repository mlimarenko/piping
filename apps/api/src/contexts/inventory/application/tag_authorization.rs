use std::future::Future;
use std::sync::{Arc, OnceLock};

use sqlx::PgConnection;
use uuid::Uuid;

use crate::contexts::access_control::application::{PolicyEvaluationService, UserApiTokenService};
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    SqlxPolicyRepository, SqlxRoleRepository, SqlxUserRepository,
};
use crate::contexts::inventory::domain::extras::tag::Tag;
use crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::{
    TagAuthorizer, load_membership_tags, lock_tag_catalog_for_usage,
};
use crate::contexts::shared::request_context::{ApiTokenScopeContext, RequestContext};
use crate::errors::AppError;

type TagPolicyEvaluator =
    PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;

/// Evaluates persisted Tag attributes through the transaction that owns their lock.
pub(crate) struct RequestTagAuthorizer<'a> {
    pub policy_evaluation: &'a TagPolicyEvaluator,
    pub user_api_tokens: &'a UserApiTokenService,
    pub context: &'a RequestContext,
    pub action: Action,
}

impl RequestTagAuthorizer<'_> {
    async fn authorize_attributes(
        &self,
        connection: &mut PgConnection,
        id: Uuid,
        mut attributes: serde_json::Value,
    ) -> Result<(), AppError> {
        attributes["object_type"] = serde_json::json!("extras.tag");
        let attributes: std::collections::HashMap<String, serde_json::Value> =
            serde_json::from_value(attributes)?;
        let resource = format!("inventory:{id}");
        self.policy_evaluation
            .ensure_access_with_attributes_on_connection(
                connection,
                self.context,
                self.action.clone(),
                ResourceType::Inventory,
                Some(&resource),
                Some(attributes.clone()),
            )
            .await?;
        if self.context.auth_method == "api_token" {
            let token_id = self
                .context
                .api_token_id
                .as_deref()
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| AppError::Auth("Invalid API token identity".into()))?;
            let user_id = self.context.require_authenticated_user_id()?;
            let token = self
                .user_api_tokens
                .ensure_current_by_id_on_connection(
                    connection,
                    token_id,
                    user_id,
                    chrono::Utc::now(),
                )
                .await?;
            let mut current = self.context.clone();
            current.api_token_scopes = token
                .scopes
                .iter()
                .map(ApiTokenScopeContext::from)
                .collect();
            current.api_token_scope_resource_override = None;
            self.policy_evaluation
                .ensure_access_with_attributes_on_connection(
                    connection,
                    &current,
                    self.action.clone(),
                    ResourceType::Inventory,
                    Some(&resource),
                    Some(attributes),
                )
                .await?;
        }
        Ok(())
    }
}

impl TagAuthorizer for RequestTagAuthorizer<'_> {
    fn authorize<'a>(
        &'a mut self,
        connection: &'a mut PgConnection,
        tag: Tag,
    ) -> futures_util::future::BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            self.authorize_attributes(connection, tag.id, serde_json::to_value(tag)?)
                .await
        })
    }
}

tokio::task_local! {
    static NATIVE_TAG_AUTHORITY: Arc<NativeTagAuthority>;
}

/// Lives only while an inventory request is dispatched. Native writers borrow
/// its authority, never its connection: each transaction authorizes on its own.
pub(crate) struct NativeTagAuthority {
    policy_evaluation: Arc<TagPolicyEvaluator>,
    user_api_tokens: Arc<UserApiTokenService>,
    context: RequestContext,
    supplied_tags: OnceLock<Vec<Uuid>>,
}

impl NativeTagAuthority {
    pub(crate) fn new(
        policy_evaluation: Arc<TagPolicyEvaluator>,
        user_api_tokens: Arc<UserApiTokenService>,
        context: RequestContext,
    ) -> Self {
        Self {
            policy_evaluation,
            user_api_tokens,
            context,
            supplied_tags: OnceLock::new(),
        }
    }

    pub(crate) async fn scope<F: Future>(self, dispatch: F) -> F::Output {
        NATIVE_TAG_AUTHORITY.scope(Arc::new(self), dispatch).await
    }
}

/// Validate the raw request once, before Option<Vec<_>> can erase explicit null.
/// Omitted tags and [] authorize no references, including on full-row updates.
pub(crate) fn retain_supplied_memberships(value: &serde_json::Value) -> Result<(), AppError> {
    let mut records = vec![value];
    if let Some(items) = value.get("items").and_then(serde_json::Value::as_array) {
        records.extend(items);
    }
    let mut supplied = std::collections::BTreeSet::new();
    for record in records {
        let Some(tags) = record.get("tags") else {
            continue;
        };
        let tags = tags.as_array().ok_or_else(|| {
            AppError::Validation("tags must be an array of canonical UUID strings, not null".into())
        })?;
        let mut seen = std::collections::HashSet::new();
        for tag in tags {
            let raw = tag
                .as_str()
                .ok_or_else(|| AppError::Validation("tags must contain UUID strings".into()))?;
            let id = Uuid::parse_str(raw)
                .map_err(|_| AppError::Validation("tags must contain UUID strings".into()))?;
            if id.to_string() != raw || !seen.insert(id) {
                return Err(AppError::Validation(
                    "tags must contain distinct canonical UUID strings".into(),
                ));
            }
            supplied.insert(id);
        }
    }
    NATIVE_TAG_AUTHORITY
        .try_with(|authority| {
            authority
                .supplied_tags
                .set(supplied.into_iter().collect())
                .map_err(|_| AppError::Internal("Inventory payload was already extracted".into()))
        })
        .map_err(|_| AppError::Internal("Inventory request authority is missing".into()))?
}

/// Called by the canonical native transaction starter, before other write locks.
/// Internal import/replay transactions retain their explicit commit authority.
pub(crate) async fn authorize_native_memberships(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), AppError> {
    let Ok(authority) = NATIVE_TAG_AUTHORITY.try_with(Arc::clone) else {
        return Ok(());
    };
    let Some(ids) = authority.supplied_tags.get().filter(|ids| !ids.is_empty()) else {
        return Ok(());
    };
    lock_tag_catalog_for_usage(transaction).await?;
    let mut tags: std::collections::HashMap<_, _> = load_membership_tags(transaction, ids)
        .await?
        .into_iter()
        .map(|tag| (tag.id, tag))
        .collect();
    let mut authorizer = RequestTagAuthorizer {
        policy_evaluation: &authority.policy_evaluation,
        user_api_tokens: &authority.user_api_tokens,
        context: &authority.context,
        action: Action::Read,
    };
    for id in ids {
        if let Some(tag) = tags.remove(id) {
            authorizer.authorize(transaction, tag).await?;
        } else {
            // A missing identity still has the caller's resource and PAT gate;
            // do not reveal existence to a principal without that READ ceiling.
            authorizer
                .authorize_attributes(transaction, *id, serde_json::json!({}))
                .await?;
            return Err(AppError::Validation("Unknown tag identity".into()));
        }
    }
    Ok(())
}
