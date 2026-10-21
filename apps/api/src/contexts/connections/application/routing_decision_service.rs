use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::connection_config::ConnConfScope;
use crate::contexts::connections::domain::{
    ConnectionProtocol, ConnectionSession, ProxyNode, ProxyNodeAssignment, RoutingSource,
    validate_proxy_base_url,
};
use crate::contexts::connections::infrastructure::ConnectionRepository;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct ConnectionRoutingDecision {
    pub proxy_node_id: Option<Uuid>,
    pub proxy_node_name: Option<String>,
    pub proxy_node_slug: Option<String>,
    pub proxy_base_url: Option<String>,
    pub routing_source: RoutingSource,
}

#[derive(Clone)]
pub struct RoutingDecisionService<R>
where
    R: ConnectionRepository,
{
    repository: Arc<R>,
}

impl<R> RoutingDecisionService<R>
where
    R: ConnectionRepository,
{
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }

    pub async fn resolve_for_session_create(
        &self,
        requested_proxy_node_id: Option<Uuid>,
        _target_host: Option<&str>,
        _protocol: &ConnectionProtocol,
        target_scopes: &[(ConnConfScope, Option<Uuid>)],
    ) -> Result<ConnectionRoutingDecision, AppError> {
        if let Some(proxy_node_id) = requested_proxy_node_id {
            let proxy_node = self
                .load_enabled_proxy_node(proxy_node_id, "request proxy override")
                .await?;
            return Ok(ConnectionRoutingDecision {
                proxy_node_id: Some(proxy_node.id),
                proxy_node_name: Some(proxy_node.name),
                proxy_node_slug: Some(proxy_node.slug),
                proxy_base_url: Some(proxy_node.base_url),
                routing_source: RoutingSource::RequestOverride,
            });
        }

        let assignments = self
            .repository
            .list_proxy_node_assignments_for_scopes(target_scopes)
            .await?;
        if let Some(assignment) = select_proxy_assignment(target_scopes, &assignments) {
            let proxy_node = self
                .load_enabled_proxy_node(assignment.proxy_node_id, "proxy assignment")
                .await?;
            return Ok(ConnectionRoutingDecision {
                proxy_node_id: Some(proxy_node.id),
                proxy_node_name: Some(proxy_node.name),
                proxy_node_slug: Some(proxy_node.slug),
                proxy_base_url: Some(proxy_node.base_url),
                routing_source: RoutingSource::ProxyAssignment,
            });
        }

        Ok(ConnectionRoutingDecision {
            proxy_node_id: None,
            proxy_node_name: None,
            proxy_node_slug: None,
            proxy_base_url: None,
            routing_source: RoutingSource::EmbeddedDefault,
        })
    }

    pub async fn resolve_for_existing_session(
        &self,
        session: &ConnectionSession,
    ) -> Result<ConnectionRoutingDecision, AppError> {
        if let Some(proxy_node_id) = session.proxy_node_id {
            let proxy_node = self
                .load_enabled_proxy_node(proxy_node_id, "session proxy routing")
                .await?;
            return Ok(ConnectionRoutingDecision {
                proxy_node_id: Some(proxy_node.id),
                proxy_node_name: Some(proxy_node.name),
                proxy_node_slug: Some(proxy_node.slug),
                proxy_base_url: Some(proxy_node.base_url),
                routing_source: session.routing_source.clone(),
            });
        }

        Ok(ConnectionRoutingDecision {
            proxy_node_id: None,
            proxy_node_name: None,
            proxy_node_slug: None,
            proxy_base_url: None,
            routing_source: session.routing_source.clone(),
        })
    }

    async fn load_enabled_proxy_node(
        &self,
        proxy_node_id: Uuid,
        source: &str,
    ) -> Result<ProxyNode, AppError> {
        let proxy_node = self
            .repository
            .get_proxy_node(proxy_node_id)
            .await?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "Proxy node not found for {}: {}",
                    source, proxy_node_id
                ))
            })?;

        if !proxy_node.enabled {
            return Err(AppError::Validation(format!(
                "Proxy node '{}' is disabled and cannot be used for {}",
                proxy_node.slug, source
            )));
        }

        validate_proxy_base_url(&proxy_node.base_url)
            .map_err(|error| AppError::Validation(error.to_string()))?;

        Ok(proxy_node)
    }
}

fn select_proxy_assignment<'a>(
    target_scopes: &[(ConnConfScope, Option<Uuid>)],
    assignments: &'a [ProxyNodeAssignment],
) -> Option<&'a ProxyNodeAssignment> {
    assignments
        .iter()
        .filter(|assignment| {
            assignment.enabled
                && target_scopes.iter().any(|(scope_type, scope_id)| {
                    assignment.scope_type == *scope_type && assignment.scope_id == *scope_id
                })
        })
        .max_by_key(|assignment| {
            (
                assignment.scope_type.specificity(),
                assignment.weight,
                assignment.id,
            )
        })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;

    fn assignment(
        id: u128,
        scope_type: ConnConfScope,
        scope_id: Option<Uuid>,
        weight: i32,
    ) -> ProxyNodeAssignment {
        ProxyNodeAssignment {
            id: Uuid::from_u128(id),
            proxy_node_id: Uuid::from_u128(1000 + id),
            scope_type,
            scope_id,
            weight,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn selects_most_specific_proxy_assignment() {
        let region_id = Uuid::from_u128(10);
        let device_id = Uuid::from_u128(20);
        let scopes = vec![
            (ConnConfScope::Global, None),
            (ConnConfScope::Region, Some(region_id)),
            (ConnConfScope::Device, Some(device_id)),
        ];
        let assignments = vec![
            assignment(1, ConnConfScope::Global, None, 100),
            assignment(2, ConnConfScope::Region, Some(region_id), 0),
            assignment(3, ConnConfScope::Device, Some(device_id), 0),
        ];

        let selected = select_proxy_assignment(&scopes, &assignments).expect("selected");

        assert_eq!(selected.id, Uuid::from_u128(3));
    }

    #[test]
    fn weight_breaks_ties_at_same_scope_specificity() {
        let role_id = Uuid::from_u128(30);
        let scopes = vec![(ConnConfScope::Role, Some(role_id))];
        let assignments = vec![
            assignment(1, ConnConfScope::Role, Some(role_id), 10),
            assignment(2, ConnConfScope::Role, Some(role_id), 20),
        ];

        let selected = select_proxy_assignment(&scopes, &assignments).expect("selected");

        assert_eq!(selected.id, Uuid::from_u128(2));
    }

    #[test]
    fn disabled_assignments_are_ignored() {
        let site_id = Uuid::from_u128(40);
        let scopes = vec![(ConnConfScope::Site, Some(site_id))];
        let mut disabled = assignment(1, ConnConfScope::Site, Some(site_id), 100);
        disabled.enabled = false;

        assert!(select_proxy_assignment(&scopes, &[disabled]).is_none());
    }
}
