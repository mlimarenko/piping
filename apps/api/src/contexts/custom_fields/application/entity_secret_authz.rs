use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use crate::contexts::access_control::application::AccessControlService;
use crate::contexts::access_control::domain::{Action, PolicyDecision, ResourceType};
use crate::contexts::access_control::infrastructure::persistence::{
    PolicyRepository, RoleRepository, UserRepository,
};
use crate::contexts::shared::request_context::RequestContext;
use crate::errors::AppError;

/// Per-entity secret authorization for custom fields.
///
/// A secret custom-field value of an inventory entity is revealed only when the
/// caller passes an attribute-based `Action::Secret` check against the entity's
/// `inventory:{entity_id}` resource. A policy MAY further constrain reveal by the
/// entity's own attributes (for example, only entities in a production site), so
/// the caller supplies the fetched entity row's attributes (`site_id`, `role_id`,
/// `tenant_id`, `status`, ...).
///
/// This reuses the existing `check_access_with_attributes` evaluator and does
/// NOT introduce a new `Action`.
///
/// # Errors
/// Propagates any [`AppError`] raised while resolving the caller's policies.
pub async fn can_read_entity_secret<U, R, P>(
    access_control: &AccessControlService<U, R, P>,
    request_context: &RequestContext,
    entity_id: Uuid,
    attributes: Value,
) -> Result<bool, AppError>
where
    U: UserRepository,
    R: RoleRepository,
    P: PolicyRepository,
{
    let resource_id = format!("inventory:{entity_id}");
    let attributes = value_to_attributes(attributes);
    let decision = access_control
        .check_access_with_attributes(
            request_context,
            Action::Secret,
            ResourceType::Inventory,
            Some(&resource_id),
            attributes,
        )
        .await?;
    Ok(decision == PolicyDecision::Allow)
}

/// Flatten a JSON object of entity attributes into the policy-context map. A
/// non-object value yields no attributes (the check then relies on the
/// resource id / action alone).
fn value_to_attributes(value: Value) -> Option<HashMap<String, Value>> {
    match value {
        Value::Object(map) => Some(map.into_iter().collect()),
        _ => None,
    }
}
