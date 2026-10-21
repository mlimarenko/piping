//! Monitoring REST routes: service discovery, exporters and assignments, metric queries and sets, host metrics, history, alert sources and quick start.
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::app::AppState;
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::errors::AppError;

mod helpers;
mod presets;
mod repository;

mod discovery;

mod exporters;

mod query_sources;

mod alert_sources;

mod metric_queries;

mod metric_sets;

mod host_metrics;

mod history;

mod quick_start;

pub use alert_sources::*;
pub use discovery::*;
pub use exporters::*;
pub use history::*;
pub use host_metrics::*;
pub use metric_queries::*;
pub use metric_sets::*;
pub use query_sources::*;
pub use quick_start::*;

pub const MONITORING_EXPORTER_ASSIGNMENT_COLUMNS:
    &[crate::contexts::shared::list_query::ListColumn] = repository::EXPORTER_ASSIGNMENT_COLUMNS;

use crate::api::access_control_helpers::build_policy_evaluation_service;
use crate::api::connection_authz_attributes::connection_target_authz_attributes_for;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::contexts::config::domain::MonitoringSdLabelGroups;
use crate::contexts::shared::request_context::RequestContext;

use crate::contexts::monitoring::application::{
    enrich_hosts_with_monitoring, fetch_discovery_hosts,
};
use crate::contexts::monitoring::domain::{DiscoveryHost, MonitoringMetricDescriptor};
use crate::contexts::secrets::domain::SECRET_KIND_NAMED;
use crate::contexts::secrets::infrastructure::SecretRepository;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

fn deserialize_string_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<StringOrVec>::deserialize(deserializer)?;
    Ok(match value {
        Some(StringOrVec::One(item)) => vec![item],
        Some(StringOrVec::Many(items)) => items,
        None => Vec::new(),
    })
}

fn parse_repeated_query_params(raw_query: Option<String>) -> HashMap<String, Vec<String>> {
    let mut params = HashMap::<String, Vec<String>>::new();
    if let Some(raw_query) = raw_query {
        for (key, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
            params
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
    }
    params
}

fn query_scalar(params: &HashMap<String, Vec<String>>, key: &str) -> Option<String> {
    params.get(key).and_then(|values| {
        values
            .iter()
            .rev()
            .find(|value| !value.trim().is_empty())
            .cloned()
    })
}

fn query_values(params: &HashMap<String, Vec<String>>, key: &str) -> Vec<String> {
    params.get(key).cloned().unwrap_or_default()
}

fn parse_optional_query_param<T>(
    params: &HashMap<String, Vec<String>>,
    key: &str,
) -> Result<Option<T>, AppError>
where
    T: FromStr,
{
    let Some(value) = query_scalar(params, key) else {
        return Ok(None);
    };
    value
        .parse::<T>()
        .map(Some)
        .map_err(|_| AppError::Validation(format!("Query parameter `{key}` has an invalid value")))
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GroupByField {
    Kind,
    Site,
    Tenant,
    Role,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetSource {
    Address,
    Name,
    Id,
    InterfaceAddresses,
    TunnelAddresses,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GroupingMode {
    PerTarget,
    SingleGroup,
    ByFields,
}

async fn ensure_monitoring_read(
    state: &AppState,
    request_context: &RequestContext,
    target_uuid: Uuid,
) -> Result<(), AppError> {
    // Attribute-aware read authorization: the target's inventory
    // facets let attribute-constrained policies (site/role/tenant/...) scope
    // monitoring reads the same way they scope connects. No protocol — a
    // metrics read has no interactive protocol.
    let attributes = connection_target_authz_attributes_for(state, target_uuid, None).await?;
    let resource_id = connection_target_resource_id(target_uuid);
    build_policy_evaluation_service(state)
        .ensure_access_with_attributes(
            request_context,
            Action::Read,
            ResourceType::ConnectionTarget,
            Some(resource_id.as_str()),
            Some(attributes),
        )
        .await
}

async fn ensure_monitoring_config_read(
    state: &AppState,
    request_context: &RequestContext,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(
            request_context,
            Action::Read,
            ResourceType::Monitoring,
            None,
        )
        .await
}

fn connection_target_resource_id(host_id: Uuid) -> String {
    format!("connection_target:{host_id}")
}

async fn fetch_host_monitoring_config(
    state: &AppState,
    host_id: Uuid,
) -> Result<DiscoveryHost, AppError> {
    let filters = DiscoveryHostFilters {
        host_id: Some(host_id),
        fetch_limit: 1,
        ..DiscoveryHostFilters::default()
    };
    // Unfiltered: every caller of this per-target config lookup sits behind
    // `ensure_monitoring_read`, which already enforced `Read × CONNECTION_TARGET`
    // for this id with its resolved attributes.
    let hosts = fetch_discovery_hosts(&state.repositories.discovery, &filters, None).await?;
    let mut hosts =
        enrich_hosts_with_monitoring(&state.db_pool, &state.settings.monitoring, hosts).await?;
    let Some(host) = hosts.pop() else {
        return Err(AppError::NotFound("Host not found".to_string()));
    };

    if host.monitoring.scrapes.is_empty() && host.monitoring.metrics.is_empty() {
        return Err(AppError::NotFound("Host not found".to_string()));
    }

    Ok(host)
}

fn select_latest_metric_descriptors(
    requested_queries: &[String],
    descriptors: &[MonitoringMetricDescriptor],
    max_queries: usize,
    empty_message: &str,
    rejected_query_message: &str,
) -> Result<LatestMetricQueries, AppError> {
    if descriptors.is_empty() {
        let requested = collect_metric_query_values(requested_queries);
        if requested.is_empty() {
            return Ok(LatestMetricQueries {
                descriptors: Vec::new(),
                message: Some(empty_message.to_string()),
            });
        }
        return Err(AppError::Validation(empty_message.to_string()));
    }

    let requested = collect_metric_query_values(requested_queries);
    let selected_descriptors = if requested.is_empty() {
        descriptors
            .iter()
            .take(max_queries)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let by_query = descriptors
            .iter()
            .map(|metric| (metric.query.as_str(), metric))
            .collect::<HashMap<_, _>>();
        if requested
            .iter()
            .any(|query| !by_query.contains_key(query.as_str()))
        {
            return Err(AppError::Validation(rejected_query_message.to_string()));
        }
        if requested.len() > max_queries {
            return Err(AppError::Validation(format!(
                "Metric query request must include at most {max_queries} queries"
            )));
        }
        requested
            .into_iter()
            .filter_map(|query| by_query.get(query.as_str()).cloned().cloned())
            .collect::<Vec<_>>()
    };
    Ok(LatestMetricQueries {
        descriptors: selected_descriptors,
        message: None,
    })
}

fn collect_metric_query_values(raw_queries: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut queries = Vec::new();

    for raw_query in raw_queries {
        let query = raw_query.trim();
        if query.is_empty() {
            continue;
        }
        if seen.insert(query.to_string()) {
            queries.push(query.to_string());
        }
    }

    queries
}

async fn ensure_monitoring_config_write(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<(), AppError> {
    build_policy_evaluation_service(state)
        .ensure_access(request_context, action, ResourceType::Monitoring, None)
        .await
}

/// Resolve an optional monitoring-write verdict without swallowing
/// infrastructure errors.
///
/// Quick-start always requires Create for its composite assignment/preset work;
/// its transaction additionally needs the Update verdict for an existing slug.
async fn monitoring_config_write_allowed(
    state: &AppState,
    request_context: &RequestContext,
    action: Action,
) -> Result<bool, AppError> {
    match ensure_monitoring_config_write(state, request_context, action).await {
        Ok(()) => Ok(true),
        Err(AppError::Forbidden(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Gate binding a named secret to a monitoring query source.
///
/// A query URL is caller controlled and the resolver sends the bound secret as
/// a bearer token to that URL. Binding is therefore disclosure-equivalent and
/// requires the canonical per-secret `Secret × SecretVariable` authority in
/// addition to the caller's applicable monitoring Create or Update authority.
/// Check authorization before loading the secret so an unauthorized caller
/// cannot distinguish an existing secret from an absent one.
async fn ensure_query_source_credential_use(
    state: &AppState,
    request_context: &RequestContext,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(secret_id) = credential_secret_id else {
        return Ok(());
    };

    // Match the secret-value routes exactly: the canonical prefixed resource id
    // preserves per-secret allow/deny decisions and the PAT scope ceiling.
    let resource_id = format!("secret_variable:{secret_id}");
    build_policy_evaluation_service(state)
        .ensure_access(
            request_context,
            Action::Secret,
            ResourceType::SecretVariable,
            Some(&resource_id),
        )
        .await?;

    let secret = state
        .repositories
        .secrets
        .get_variable_by_id(secret_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation(
                "credential_ref must identify an existing named secret".to_string(),
            )
        })?;
    if secret.kind != SECRET_KIND_NAMED {
        return Err(AppError::Validation(
            "credential_ref must identify an existing named secret".to_string(),
        ));
    }
    if secret.is_protected {
        return Err(AppError::Forbidden(
            "Protected named secrets cannot be used by monitoring query sources".to_string(),
        ));
    }

    Ok(())
}

/// Gate binding a named secret to a monitoring alert source.
///
/// An alert source URL is caller controlled and the poller sends the bound
/// secret as a bearer token to that URL. Binding is therefore
/// disclosure-equivalent and requires the canonical per-secret
/// `Secret × SecretVariable` authority in addition to the caller's applicable
/// monitoring Create or Update authority. Check authorization before loading
/// the secret so an unauthorized caller cannot distinguish an existing secret
/// from an absent one.
async fn ensure_alert_source_credential_use(
    state: &AppState,
    request_context: &RequestContext,
    credential_secret_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(secret_id) = credential_secret_id else {
        return Ok(());
    };

    // Match the secret-value routes exactly: the canonical prefixed resource id
    // preserves per-secret allow/deny decisions and the PAT scope ceiling.
    let resource_id = format!("secret_variable:{secret_id}");
    build_policy_evaluation_service(state)
        .ensure_access(
            request_context,
            Action::Secret,
            ResourceType::SecretVariable,
            Some(&resource_id),
        )
        .await?;

    let secret = state
        .repositories
        .secrets
        .get_variable_by_id(secret_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation(
                "credential_ref must identify an existing named secret".to_string(),
            )
        })?;
    if secret.kind != SECRET_KIND_NAMED {
        return Err(AppError::Validation(
            "credential_ref must identify an existing named secret".to_string(),
        ));
    }
    if secret.is_protected {
        return Err(AppError::Forbidden(
            "Protected named secrets cannot be used by monitoring alert sources".to_string(),
        ));
    }

    Ok(())
}

/// Toggle of which connection-target metadata label groups the service-discovery output emits.
///
/// Target basics (host_id/name/kind/address) are always emitted and are not part
/// of this set.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SdLabelGroupsDto {
    pub location: bool,
    pub tenant: bool,
    pub role: bool,
    pub status: bool,
    pub hardware: bool,
    pub network: bool,
    pub contacts: bool,
    pub description: bool,
    pub identifiers: bool,
    #[serde(default = "default_sd_statuses")]
    pub device_statuses: Vec<String>,
    #[serde(default = "default_sd_statuses")]
    pub site_statuses: Vec<String>,
}

fn default_sd_statuses() -> Vec<String> {
    vec!["active".to_string()]
}

impl From<MonitoringSdLabelGroups> for SdLabelGroupsDto {
    fn from(groups: MonitoringSdLabelGroups) -> Self {
        Self {
            location: groups.location,
            tenant: groups.tenant,
            role: groups.role,
            status: groups.status,
            hardware: groups.hardware,
            network: groups.network,
            contacts: groups.contacts,
            description: groups.description,
            identifiers: groups.identifiers,
            device_statuses: groups.device_statuses,
            site_statuses: groups.site_statuses,
        }
    }
}

impl From<SdLabelGroupsDto> for MonitoringSdLabelGroups {
    fn from(dto: SdLabelGroupsDto) -> Self {
        Self {
            location: dto.location,
            tenant: dto.tenant,
            role: dto.role,
            status: dto.status,
            hardware: dto.hardware,
            network: dto.network,
            contacts: dto.contacts,
            description: dto.description,
            identifiers: dto.identifiers,
            device_statuses: dto.device_statuses,
            site_statuses: dto.site_statuses,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SdLabelSettingsResponse {
    pub groups: SdLabelGroupsDto,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct SdLabelSettingsPayload {
    pub groups: SdLabelGroupsDto,
}

// ===========================================================================
// Inspector-side CRUD: query sources, metric queries, metric sets, assignments
// ===========================================================================

const MONITORING_SCOPE_TYPES: [&str; 5] = ["global", "site", "tenant", "role", "connection_target"];
const MONITORING_SOURCE_TYPES: [&str; 3] = ["prometheus", "victoriametrics", "zabbix"];

/// Validate a scope_type/scope_id pair against the canonical scope rule
/// (`global` ⟺ `scope_id is None`). Returns the normalized lowercase scope_type.
fn validate_monitoring_scope(scope_type: &str, scope_id: Option<Uuid>) -> Result<String, AppError> {
    let scope_type = scope_type.trim().to_ascii_lowercase();
    if !MONITORING_SCOPE_TYPES.contains(&scope_type.as_str()) {
        return Err(AppError::Validation(format!(
            "scope_type must be one of: {}",
            MONITORING_SCOPE_TYPES.join(", ")
        )));
    }
    if scope_type == "global" {
        if scope_id.is_some() {
            return Err(AppError::Validation(
                "scope_id must be omitted when scope_type is `global`".to_string(),
            ));
        }
    } else if scope_id.is_none() {
        return Err(AppError::Validation(format!(
            "scope_id is required when scope_type is `{scope_type}`"
        )));
    }
    Ok(scope_type)
}

/// Validate that the given value is one of `allowed`, returning the normalized
/// lowercase value.
fn validate_enum_value(field: &str, value: &str, allowed: &[&str]) -> Result<String, AppError> {
    let value = value.trim().to_ascii_lowercase();
    if !allowed.contains(&value.as_str()) {
        return Err(AppError::Validation(format!(
            "{field} must be one of: {}",
            allowed.join(", ")
        )));
    }
    Ok(value)
}

/// Coerce an optional JSON value into a JSON object, defaulting to `{}`.
fn validate_json_object(field: &str, value: Option<Value>) -> Result<Value, AppError> {
    match value {
        Some(Value::Object(map)) => Ok(Value::Object(map)),
        None | Some(Value::Null) => Ok(Value::Object(serde_json::Map::new())),
        Some(_) => Err(AppError::Validation(format!(
            "{field} must be a JSON object"
        ))),
    }
}

/// Deserialize the public named-secret reference grammar exactly once at the
/// REST boundary. `Uuid::parse_str` deliberately accepts additional legacy
/// spellings (compact, braced, and `urn:uuid:`), but query-source APIs accept
/// only canonical lower-case, hyphenated UUID text. The migration is the sole
/// compatibility path for those historical stored spellings.
fn deserialize_optional_canonical_uuid<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|raw| {
            let parsed = Uuid::parse_str(&raw)
                .map_err(|_| serde::de::Error::custom("credential_ref must be a canonical UUID"))?;
            if raw != parsed.hyphenated().to_string() {
                return Err(serde::de::Error::custom(
                    "credential_ref must be a canonical UUID",
                ));
            }
            Ok(parsed)
        })
        .transpose()
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MonitoringScopeListQuery {
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    pub enabled: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub query_source_id: Option<String>,
    pub alert_source_id: Option<String>,
    pub metric_set_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::contexts::inventory::domain::DiscoveryInterfaceAddress;
    use crate::contexts::monitoring::domain::{
        HostMonitoringConfig, LatestMetricSourceInfo, MonitoringScrapeConfig,
        ResolvedMetricQuerySource,
    };

    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn metric_test_private_resolution_is_provenance_and_authority_gated() {
        // A URL matching an admin-created source is trusted on the
        // monitoring-config branch...
        assert!(metric_test_allows_private_resolution(true, None));
        // ...but the target-scoped branch keeps the strict guard even for a
        // matched source, so it cannot reach an internal backend with the
        // source's admin credential.
        assert!(!metric_test_allows_private_resolution(
            true,
            Some(Uuid::nil())
        ));
        // An ad-hoc URL never earns the bypass.
        assert!(!metric_test_allows_private_resolution(false, None));
        assert!(!metric_test_allows_private_resolution(
            false,
            Some(Uuid::nil())
        ));
    }

    fn discovery_options() -> DiscoveryRequestOptions {
        DiscoveryRequestOptions {
            kind_label: None,
            target_source: TargetSource::Address,
            grouping: GroupingMode::PerTarget,
            group_fields: Vec::new(),
            default_port: Some(9100),
            default_scheme: "http".to_string(),
            default_metrics_path: "/metrics".to_string(),
            default_job: Some("node".to_string()),
            device_job: None,
            vm_job: None,
            dedup_targets: true,
            page: DiscoveryPage {
                limit: 100,
                offset: 0,
            },
            filters: DiscoveryHostFilters {
                host_id: None,
                kind: None,
                site_id: None,
                tenant_id: None,
                role_id: None,
                status: None,
                site_status: None,
                allowed_statuses: None,
                allowed_site_statuses: None,
                search: None,
                fetch_limit: 100,
                fetch_offset: 0,
            },
            extra_labels: HashMap::new(),
            with_port: true,
        }
    }

    fn discovery_host() -> DiscoveryHost {
        DiscoveryHost {
            host_id: Uuid::nil(),
            host_name: "db-01".to_string(),
            host_address: "10.0.0.10".to_string(),
            kind: "virtual_machine".to_string(),
            site_id: None,
            site_name: None,
            site_slug: None,
            tenant_id: None,
            tenant_name: None,
            tenant_slug: None,
            role_id: None,
            role_name: None,
            role_slug: None,
            status: Some("active".to_string()),
            platform_name: None,
            model: None,
            manufacturer: None,
            serial: None,
            asset_tag: None,
            primary_ip4: None,
            primary_ip6: None,
            interfaces: None,
            interface_addresses: Vec::new(),
            tunnel_addresses: Vec::new(),
            contacts: None,
            description: None,
            comments: None,
            site_facility: None,
            site_address: None,
            site_region: None,
            site_group: None,
            tenant_group: None,
            rack: None,
            cluster: None,
            monitoring: HostMonitoringConfig {
                scrapes: vec![
                    MonitoringScrapeConfig {
                        name: Some("node".to_string()),
                        job: Some("node-exporter".to_string()),
                        ..MonitoringScrapeConfig::default()
                    },
                    MonitoringScrapeConfig {
                        name: Some("postgres".to_string()),
                        target: Some("db.internal".to_string()),
                        job: Some("postgres-exporter".to_string()),
                        metrics_path: Some("/pgmetrics".to_string()),
                        port: Some(9187),
                        ..MonitoringScrapeConfig::default()
                    },
                ],
                ..HostMonitoringConfig::default()
            },
        }
    }

    fn metric_descriptors() -> Vec<MonitoringMetricDescriptor> {
        vec![
            MonitoringMetricDescriptor {
                name: "load1".to_string(),
                label: Some("Load 1m".to_string()),
                query: "node_load1{piping_host_name=\"db-01\"}".to_string(),
                unit: Some("load".to_string()),
                value_type: Some("number".to_string()),
                widget_type: Some("stat".to_string()),
                description: None,
                source_type: Some("prometheus".to_string()),
                query_type: Some("promql".to_string()),
                options: Value::Object(Default::default()),
                show_in_list: false,
                list_render: "auto".to_string(),
                metric_query_id: None,
                category: None,
                semantic: None,
            },
            MonitoringMetricDescriptor {
                name: "pg_up".to_string(),
                label: None,
                query: "pg_up{piping_host_name=\"db-01\"}".to_string(),
                unit: None,
                value_type: Some("state".to_string()),
                widget_type: Some("badge".to_string()),
                description: None,
                source_type: Some("prometheus".to_string()),
                query_type: Some("promql".to_string()),
                options: Value::Object(Default::default()),
                show_in_list: false,
                list_render: "auto".to_string(),
                metric_query_id: None,
                category: None,
                semantic: None,
            },
        ]
    }

    #[test]
    fn a_reached_provider_that_errors_still_reports_the_step_clamp() {
        // The step-clamp warning tells the user their chart is coarser than
        // they asked for. It was carried on the Zabbix error path and dropped
        // on the Prometheus one, so the same failure explained itself on one
        // backend and stayed silent on the other. Both now answer from
        // `provider_error_history`, so the warning survives either way.
        let clamp = Some("Step was auto-raised from 60s to 300s".to_string());
        let source = LatestMetricSourceInfo {
            name: Some("prom-1".to_string()),
            source_type: Some("prometheus".to_string()),
            configured_from: "site".to_string(),
            query_url: "http://prom.internal/api/v1/query".to_string(),
            credential_ref: None,
        };

        let response = provider_error_history(ProviderErrorHistory {
            query: "up",
            executed_query: "up{host=\"a\"}".to_string(),
            unit: Some("percent".to_string()),
            value_type: Some("number".to_string()),
            source,
            clamp_message: clamp.clone(),
            error: "upstream refused the connection".to_string(),
        });

        assert_eq!(
            response.message, clamp,
            "a provider error must not swallow the step-clamp warning"
        );
        assert_eq!(
            response.error.as_deref(),
            Some("upstream refused the connection")
        );
        assert!(response.configured, "the source was configured and reached");
        assert!(response.series.is_empty(), "an error carries no series");
    }

    #[test]
    fn connection_target_resource_id_is_canonical_for_scope_matching() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();

        assert_eq!(
            connection_target_resource_id(host_id),
            "connection_target:11111111-1111-1111-1111-111111111111"
        );
    }

    #[test]
    fn service_discovery_query_parser_accepts_repeated_vector_keys() {
        let query = parse_service_discovery_query(Some(
            "label=env%3Dprod&label=team%3Aplatform&search=db&limit=50".to_string(),
        ))
        .expect("query");

        assert_eq!(query.search, Some("db".to_string()));
        assert_eq!(query.limit, Some(50));
        assert_eq!(
            query.label,
            vec!["env=prod".to_string(), "team:platform".to_string()]
        );
    }

    #[test]
    fn service_discovery_query_parser_treats_empty_input_as_absent_options() {
        let query = parse_service_discovery_query(None).expect("empty query");

        assert!(query.token.is_none());
        assert!(query.port.is_none());
        assert!(query.site_id.is_none());
        assert!(query.label.is_empty());
    }

    #[test]
    fn service_discovery_query_parser_uses_the_last_non_empty_scalar() {
        let query = parse_service_discovery_query(Some(
            "search=first&search=%20%20&search=last".to_string(),
        ))
        .expect("repeated scalar query");

        assert_eq!(query.search.as_deref(), Some("last"));
    }

    #[test]
    fn service_discovery_query_parser_rejects_a_port_above_u16() {
        let error = parse_service_discovery_query(Some("port=65536".to_string()))
            .expect_err("overflowing port must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "Query parameter `port` has an invalid value"
        ));
    }

    #[test]
    fn latest_metrics_query_parser_accepts_repeated_query_keys() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let query = parse_latest_metrics_query(Some(format!(
            "target_id={host_id}&query=up&query=node_load1"
        )))
        .expect("query");

        assert_eq!(query.target_id, Some(host_id));
        assert_eq!(
            query.query,
            vec!["up".to_string(), "node_load1".to_string()]
        );
    }

    #[test]
    fn latest_metrics_query_parser_rejects_a_malformed_target_id() {
        let error = parse_latest_metrics_query(Some("target_id=not-a-uuid&query=up".to_string()))
            .expect_err("malformed target id must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "Query parameter `target_id` has an invalid value"
        ));
    }

    #[test]
    fn host_labels_query_ignores_empty_values_and_trims_ids() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let ids = parse_host_labels_query(Some(format!(
            "host_id=&host_id=%20{host_id}%20&host_id=%20"
        )))
        .expect("host labels query");

        assert_eq!(ids, vec![host_id]);
    }

    #[test]
    fn host_labels_query_rejects_a_malformed_non_empty_id() {
        let error = parse_host_labels_query(Some("host_id=%20bad-id%20".to_string()))
            .expect_err("malformed host id must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "Query parameter `host_id` has an invalid value"
        ));
    }

    #[test]
    fn exporter_catalog_request_normalizes_defaults_and_labels() {
        let input = validate_exporter_catalog(ExporterCatalogRequest {
            name: "  NGINX Vhost Exporter  ".to_string(),
            slug: " NGINX_Vhost.Exporter ".to_string(),
            exporter_type: None,
            description: Some("  vhost scrape profile  ".to_string()),
            default_job: None,
            default_scheme: Some("HTTP".to_string()),
            default_port: Some(9113),
            default_metrics_path: Some("metrics".to_string()),
            default_target_source: None,
            labels: Some(serde_json::json!({ "component": "nginx" })),
            enabled: None,
        })
        .expect("valid exporter catalog request");

        assert_eq!(input.slug, "nginx-vhost-exporter");
        assert_eq!(input.name, "nginx vhost exporter");
        assert_eq!(input.exporter_type, "nginx-vhost-exporter");
        assert_eq!(input.description.as_deref(), Some("vhost scrape profile"));
        assert_eq!(input.default_job, "nginx-vhost-exporter");
        assert_eq!(input.default_scheme, "http");
        assert_eq!(input.default_port, Some(9113));
        assert_eq!(input.default_metrics_path, "/metrics");
        assert_eq!(input.default_target_source, "host_address");
        assert_eq!(input.labels, serde_json::json!({ "component": "nginx" }));
        assert!(input.enabled);
    }

    #[test]
    fn metric_query_collection_preserves_promql_commas() {
        let queries = collect_metric_query_values(&[
            "sum by(instance) (rate(http_requests_total{status=~\"5..\",job=\"api\"}[5m]))"
                .to_string(),
            "up".to_string(),
            "up".to_string(),
        ]);

        assert_eq!(
            queries,
            vec![
                "sum by(instance) (rate(http_requests_total{status=~\"5..\",job=\"api\"}[5m]))"
                    .to_string(),
                "up".to_string()
            ]
        );
    }

    #[test]
    fn build_discovered_targets_expands_multiple_scrapes() {
        let targets = build_discovered_targets(vec![discovery_host()], &discovery_options());

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].target, "10.0.0.10:9100");
        assert_eq!(
            targets[0].labels.get("job").map(String::as_str),
            Some("node-exporter")
        );
        assert_eq!(
            targets[0].labels.get("piping_scrape").map(String::as_str),
            Some("node")
        );
        assert_eq!(targets[1].target, "db.internal:9187");
        assert_eq!(
            targets[1]
                .labels
                .get("__metrics_path__")
                .map(String::as_str),
            Some("/pgmetrics")
        );
        assert_eq!(
            targets[1].labels.get("job").map(String::as_str),
            Some("postgres-exporter")
        );
    }

    #[test]
    fn build_discovered_targets_preserves_label_override_precedence() {
        let mut options = discovery_options();
        options
            .extra_labels
            .insert("__scheme__".to_string(), "https".to_string());
        options
            .extra_labels
            .insert("job".to_string(), "request-job".to_string());
        let mut host = discovery_host();
        host.monitoring.scrapes[0]
            .labels
            .insert("job".to_string(), "scrape-job".to_string());

        let targets = build_discovered_targets(vec![host], &options);

        assert_eq!(targets[0].labels["__scheme__"], "https");
        assert_eq!(targets[0].labels["job"], "scrape-job");
    }

    #[test]
    fn build_discovered_targets_always_emits_stable_target_id_label() {
        let targets = build_discovered_targets(vec![discovery_host()], &discovery_options());
        let groups = build_target_groups(
            targets,
            GroupingMode::PerTarget,
            &[],
            true,
            &MonitoringSdLabelGroups::default(),
        );

        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|group| {
            group
                .labels
                .get("piping_target_id")
                .is_some_and(|value| value == "00000000-0000-0000-0000-000000000000")
        }));
        assert!(
            groups
                .iter()
                .all(|group| !group.labels.contains_key("piping_host_id"))
        );
    }

    #[test]
    fn build_discovered_targets_without_exporter_assignments_is_empty() {
        let mut host = discovery_host();
        host.monitoring.scrapes.clear();

        let targets = build_discovered_targets(vec![host], &discovery_options());

        assert!(targets.is_empty());
    }

    #[test]
    fn build_discovered_targets_fans_out_interface_addresses() {
        let mut host = discovery_host();
        host.interface_addresses = vec![
            DiscoveryInterfaceAddress {
                name: "internal".to_string(),
                address: "10.0.0.1".to_string(),
                channel_address: None,
            },
            DiscoveryInterfaceAddress {
                name: "main".to_string(),
                address: "198.51.100.10".to_string(),
                channel_address: None,
            },
        ];
        host.monitoring.scrapes = vec![MonitoringScrapeConfig {
            name: Some("blackbox".to_string()),
            job: Some("blackbox".to_string()),
            target_source: Some("interface_addresses".to_string()),
            ..MonitoringScrapeConfig::default()
        }];

        let targets = build_discovered_targets(vec![host], &discovery_options());

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].target, "10.0.0.1:9100");
        assert_eq!(targets[1].target, "198.51.100.10:9100");
        assert_eq!(
            targets[0]
                .labels
                .get("piping_target_address")
                .map(String::as_str),
            Some("10.0.0.1")
        );
        assert_eq!(
            targets[0]
                .labels
                .get("piping_interface")
                .map(String::as_str),
            Some("internal")
        );
        assert_eq!(
            targets[1]
                .labels
                .get("piping_interface")
                .map(String::as_str),
            Some("main")
        );
    }

    #[test]
    fn target_source_for_scrape_understands_tunnel_addresses() {
        let scrape = MonitoringScrapeConfig {
            target_source: Some("tunnel_addresses".to_string()),
            ..MonitoringScrapeConfig::default()
        };
        assert_eq!(
            target_source_for_scrape(&scrape, TargetSource::Address),
            TargetSource::TunnelAddresses
        );

        let scrape = MonitoringScrapeConfig {
            target_source: Some("tunnels".to_string()),
            ..MonitoringScrapeConfig::default()
        };
        assert_eq!(
            target_source_for_scrape(&scrape, TargetSource::Address),
            TargetSource::TunnelAddresses
        );
    }

    #[test]
    fn build_discovered_targets_fans_out_tunnel_addresses() {
        let mut host = discovery_host();
        host.contacts = Some("Operations (device) +15550101010 <ops@example.test>".to_string());
        host.tunnel_addresses = vec![
            DiscoveryInterfaceAddress {
                name: "primary".to_string(),
                address: "198.51.100.1".to_string(),
                channel_address: Some("192.0.2.10".to_string()),
            },
            DiscoveryInterfaceAddress {
                name: "backup".to_string(),
                address: "203.0.113.1".to_string(),
                channel_address: Some("192.0.2.20".to_string()),
            },
        ];
        host.monitoring.scrapes = vec![MonitoringScrapeConfig {
            name: Some("blackbox".to_string()),
            job: Some("blackbox".to_string()),
            target_source: Some("tunnel_addresses".to_string()),
            ..MonitoringScrapeConfig::default()
        }];

        let targets = build_discovered_targets(vec![host], &discovery_options());

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].target, "198.51.100.1:9100");
        assert_eq!(targets[1].target, "203.0.113.1:9100");
        assert_eq!(
            targets[0]
                .labels
                .get("piping_target_address")
                .map(String::as_str),
            Some("198.51.100.1")
        );
        assert_eq!(
            targets[0]
                .labels
                .get("piping_interface")
                .map(String::as_str),
            Some("primary")
        );
        assert_eq!(
            targets[0]
                .labels
                .get("piping_channel_address")
                .map(String::as_str),
            Some("192.0.2.10")
        );
        assert_eq!(
            targets[1]
                .labels
                .get("piping_target_address")
                .map(String::as_str),
            Some("203.0.113.1")
        );
        assert_eq!(
            targets[1]
                .labels
                .get("piping_interface")
                .map(String::as_str),
            Some("backup")
        );
        assert_eq!(
            targets[1]
                .labels
                .get("piping_channel_address")
                .map(String::as_str),
            Some("192.0.2.20")
        );

        let groups = build_target_groups(
            targets,
            GroupingMode::PerTarget,
            &[],
            true,
            &MonitoringSdLabelGroups::default(),
        );
        assert!(groups.iter().all(|group| {
            group.labels.get("piping_contacts").map(String::as_str)
                == Some("Operations (device) +15550101010 <ops@example.test>")
        }));
    }

    #[test]
    fn build_discovered_targets_with_interface_source_and_no_interfaces_is_empty() {
        let mut options = discovery_options();
        options.target_source = TargetSource::InterfaceAddresses;
        let mut host = discovery_host();
        host.monitoring.scrapes = vec![MonitoringScrapeConfig {
            name: Some("blackbox".to_string()),
            job: Some("blackbox".to_string()),
            ..MonitoringScrapeConfig::default()
        }];

        let targets = build_discovered_targets(vec![host], &options);

        assert!(targets.is_empty());
    }

    #[test]
    fn single_group_omits_target_specific_interface_labels() {
        let mut host = discovery_host();
        host.interface_addresses = vec![
            DiscoveryInterfaceAddress {
                name: "internal".to_string(),
                address: "10.0.0.1".to_string(),
                channel_address: None,
            },
            DiscoveryInterfaceAddress {
                name: "main".to_string(),
                address: "198.51.100.10".to_string(),
                channel_address: None,
            },
        ];
        host.monitoring.scrapes = vec![MonitoringScrapeConfig {
            name: Some("blackbox".to_string()),
            job: Some("blackbox".to_string()),
            target_source: Some("interface_addresses".to_string()),
            ..MonitoringScrapeConfig::default()
        }];

        let targets = build_discovered_targets(vec![host], &discovery_options());
        let groups = build_target_groups(
            targets,
            GroupingMode::SingleGroup,
            &[],
            true,
            &MonitoringSdLabelGroups::default(),
        );

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].targets.len(), 2);
        assert!(!groups[0].labels.contains_key("piping_target_address"));
        assert!(!groups[0].labels.contains_key("piping_interface"));
    }

    #[test]
    fn service_discovery_hosts_can_include_metrics_only_configs() {
        let db_host = discovery_host();

        let mut web_host = discovery_host();
        web_host.host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        web_host.host_name = "web-01".to_string();
        web_host.host_address = "10.0.0.20".to_string();
        web_host.monitoring.scrapes.clear();
        web_host.monitoring.metrics = metric_descriptors();

        let hosts = vec![db_host, web_host];
        let targets = build_discovered_targets(hosts.clone(), &discovery_options());

        assert_eq!(targets.len(), 2);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].host_name, "db-01");
        assert_eq!(hosts[1].host_name, "web-01");
    }

    #[test]
    fn target_scoped_latest_defaults_to_configured_descriptors() {
        let selected = select_latest_metric_descriptors(
            &[],
            &metric_descriptors(),
            1,
            "Connection target has no configured metric descriptors",
            "Connection-target metric queries must match configured connection target metric descriptors",
        )
        .expect("queries");

        assert_eq!(
            selected.descriptors[0].query,
            "node_load1{piping_host_name=\"db-01\"}"
        );
        assert!(selected.message.is_none());
    }

    #[test]
    fn explicit_latest_query_list_rejects_limit_overflow_instead_of_truncating() {
        let descriptors = metric_descriptors();
        let queries = descriptors
            .iter()
            .map(|descriptor| descriptor.query.clone())
            .collect::<Vec<_>>();

        let error = select_latest_metric_descriptors(
            &queries,
            &descriptors,
            1,
            "Connection target has no configured metric descriptors",
            "Connection-target metric queries must match configured connection target metric descriptors",
        )
        .expect_err("explicit overflow should fail");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn target_scoped_latest_rejects_unconfigured_query() {
        let error = select_latest_metric_descriptors(
            &["up".to_string()],
            &metric_descriptors(),
            10,
            "Connection target has no configured metric descriptors",
            "Connection-target metric queries must match configured connection target metric descriptors",
        )
        .expect_err("unconfigured query must fail");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn global_latest_rejects_unconfigured_query() {
        let error = select_latest_metric_descriptors(
            &["up".to_string()],
            &metric_descriptors(),
            10,
            "No configured metric descriptors are available",
            "Global metric queries must match configured metric descriptors",
        )
        .expect_err("unconfigured query must fail");

        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn target_scoped_latest_empty_descriptors_do_not_fall_back_to_global_up() {
        let selected = select_latest_metric_descriptors(
            &[],
            &[],
            10,
            "Connection target has no configured metric descriptors",
            "Connection-target metric queries must match configured connection target metric descriptors",
        )
        .expect("empty descriptors");

        assert!(selected.descriptors.is_empty());
        assert!(selected.message.is_some());
    }

    fn metric_query_payload(
        source_type: &str,
        query_type: &str,
        query: &str,
    ) -> MetricQueryRequest {
        MetricQueryRequest {
            exporter_id: None,
            source_type: Some(source_type.to_string()),
            query_type: Some(query_type.to_string()),
            value_type: Some("number".to_string()),
            widget_type: Some("stat".to_string()),
            name: "CPU Busy".to_string(),
            slug: "cpu-busy".to_string(),
            label: Some("CPU busy".to_string()),
            query: query.to_string(),
            unit: Some("%".to_string()),
            description: None,
            default_show_in_inspector: Some(true),
            default_show_in_list: false,
            sort_order: Some(10),
            options: Some(serde_json::json!({})),
            enabled: Some(true),
            category: None,
            semantic: None,
        }
    }

    fn query_source_payload(query_url: &str) -> QuerySourceRequest {
        QuerySourceRequest {
            name: "VictoriaMetrics".to_string(),
            slug: "victoriametrics".to_string(),
            source_type: Some("victoriametrics".to_string()),
            query_url: query_url.to_string(),
            credential_ref: None,
            timeout_ms: Some(3000),
            options: Some(serde_json::json!({})),
            enabled: Some(true),
        }
    }

    #[test]
    fn query_source_validation_rejects_local_and_metadata_targets() {
        for url in [
            "http://127.0.0.1:8428/api/v1/query",
            "http://localhost:8428/api/v1/query",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]:8428/api/v1/query",
            "http://[::ffff:127.0.0.1]:8428/api/v1/query",
            "http://[::ffff:169.254.169.254]/latest/meta-data",
            "file:///tmp/metrics",
        ] {
            assert!(
                matches!(
                    validate_query_source(query_source_payload(url)),
                    Err(AppError::Validation(_))
                ),
                "{url} should be rejected"
            );
        }
    }

    #[test]
    fn query_source_validation_allows_named_monitoring_backends() {
        for url in [
            "http://victoriametrics:8428/api/v1/query",
            "https://victoria.piping.space/api/v1/query",
            "https://zabbix.example/api_jsonrpc.php",
        ] {
            assert!(
                validate_query_source(query_source_payload(url)).is_ok(),
                "{url} should be accepted"
            );
        }
    }

    #[test]
    fn metric_query_validation_accepts_compatible_query_types() {
        assert!(
            validate_metric_query(metric_query_payload(
                "prometheus",
                "promql",
                r#"up{piping_host_name="$host"}"#
            ))
            .is_ok()
        );
        assert!(
            validate_metric_query(metric_query_payload(
                "victoriametrics",
                "promql",
                r#"up{piping_host_name="$host"}"#
            ))
            .is_ok()
        );
        assert!(
            validate_metric_query(metric_query_payload(
                "zabbix",
                "zabbix_item_key",
                "system.cpu.load[percpu,avg1]"
            ))
            .is_ok()
        );
    }

    #[test]
    fn metric_query_validation_rejects_incompatible_query_types() {
        assert!(matches!(
            validate_metric_query(metric_query_payload(
                "prometheus",
                "zabbix_item_key",
                "agent.ping"
            )),
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            validate_metric_query(metric_query_payload(
                "zabbix",
                "promql",
                r#"up{piping_host_name="$host"}"#
            )),
            Err(AppError::Validation(_))
        ));
    }

    #[test]
    fn metric_query_validation_preserves_query_case() {
        let query = r#"SomeMetricName{LabelName="ProdHostA",job="node"}"#;
        let validated = validate_metric_query(metric_query_payload("prometheus", "promql", query))
            .expect("valid metric query");

        assert_eq!(validated.query, query);
    }

    #[test]
    fn metric_history_query_parser_decodes_all_fields_and_uses_last_query() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let query = parse_metric_history_query(Some(format!(
            "target_id={host_id}&query=old&query=rate%28up%5B5m%5D%29&start=10.5&end=70.5&step=15"
        )))
        .expect("history query");

        assert_eq!(query.target_id, host_id);
        assert_eq!(query.query, "rate(up[5m])");
        assert_eq!(query.start, 10.5);
        assert_eq!(query.end, 70.5);
        assert_eq!(query.step, Some(15));
    }

    #[test]
    fn metric_history_query_parser_rejects_missing_required_query() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let error = parse_metric_history_query(Some(format!(
            "target_id={host_id}&query=%20%20&start=10&end=20"
        )))
        .expect_err("blank query must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "Query parameter `query` is required"
        ));
    }

    #[test]
    fn metric_history_query_parser_rejects_negative_step_for_unsigned_field() {
        let host_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let error = parse_metric_history_query(Some(format!(
            "target_id={host_id}&query=up&start=10&end=20&step=-1"
        )))
        .expect_err("negative step must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message)
                if message == "Query parameter `step` has an invalid value"
        ));
    }

    // ─────────────────────────────────────────────────────────────────────
    // prometheus_range_url
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn prometheus_range_url_replaces_query_suffix() {
        assert_eq!(
            prometheus_range_url("http://vm:9090/api/v1/query"),
            "http://vm:9090/api/v1/query_range"
        );
    }

    #[test]
    fn prometheus_range_url_keeps_existing_range_suffix() {
        assert_eq!(
            prometheus_range_url("http://vm:9090/api/v1/query_range"),
            "http://vm:9090/api/v1/query_range"
        );
    }

    #[test]
    fn prometheus_range_url_appends_to_bare_base() {
        assert_eq!(
            prometheus_range_url("http://vm:9090"),
            "http://vm:9090/api/v1/query_range"
        );
    }

    #[test]
    fn prometheus_range_url_strips_trailing_slash_before_appending() {
        assert_eq!(
            prometheus_range_url("http://vm:9090/"),
            "http://vm:9090/api/v1/query_range"
        );
    }

    #[test]
    fn prometheus_range_request_url_encodes_query_and_numeric_bounds() {
        let url = prometheus_range_request_url(
            "https://vm.example/api/v1/query?tenant=core",
            "rate(node_cpu_seconds_total{instance=\"db-01:9100\"}[5m])",
            10.5,
            70.5,
            15,
        )
        .expect("range URL");

        assert_eq!(url.path(), "/api/v1/query_range");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("tenant".into(), "core".into()),
                (
                    "query".into(),
                    "rate(node_cpu_seconds_total{instance=\"db-01:9100\"}[5m])".into(),
                ),
                ("start".into(), "10.5".into()),
                ("end".into(), "70.5".into()),
                ("step".into(), "15".into()),
            ]
        );
    }

    #[test]
    fn prometheus_range_response_surfaces_provider_error_and_http_status() {
        let provider_body = serde_json::to_vec(&serde_json::json!({
            "status": "error",
            "error": "storage unavailable"
        }))
        .unwrap();
        assert_eq!(
            decode_prometheus_range_response(StatusCode::SERVICE_UNAVAILABLE, &provider_body)
                .unwrap_err(),
            "storage unavailable"
        );

        let fallback_body = serde_json::to_vec(&serde_json::json!({ "status": "error" })).unwrap();
        assert_eq!(
            decode_prometheus_range_response(StatusCode::BAD_GATEWAY, &fallback_body).unwrap_err(),
            "HTTP 502 Bad Gateway"
        );
    }

    #[test]
    fn prometheus_range_response_rejects_malformed_json() {
        let error = decode_prometheus_range_response(StatusCode::OK, br#"{"status":]"#)
            .expect_err("malformed response must be rejected");

        assert!(!error.is_empty());
    }

    #[test]
    fn zabbix_history_params_parse_value_type_and_request_truncation_sentinel() {
        let params = zabbix_history_params(
            &serde_json::json!({ "itemid": "1001", "value_type": " 3 " }),
            10.9,
            70.9,
            100,
        )
        .expect("history params");

        assert_eq!(
            params,
            serde_json::json!({
                "output": "extend",
                "history": 3,
                "itemids": ["1001"],
                "time_from": 10,
                "time_till": 70,
                "sortfield": "clock",
                "sortorder": "ASC",
                "limit": 401,
            })
        );
    }

    #[test]
    fn zabbix_history_params_default_bad_value_type_and_saturate_limit() {
        let params = zabbix_history_params(
            &serde_json::json!({ "itemid": "1001", "value_type": "bad" }),
            0.0,
            1.0,
            usize::MAX,
        )
        .expect("history params");

        assert_eq!(params["history"], 0);
        assert_eq!(params["limit"], serde_json::json!(usize::MAX));
    }

    #[test]
    fn zabbix_history_params_reject_item_without_id() {
        let error = zabbix_history_params(&serde_json::json!({}), 0.0, 1.0, 100)
            .expect_err("item id is required");

        assert_eq!(error, "Zabbix item did not include an itemid");
    }

    // ─────────────────────────────────────────────────────────────────────
    // parse_prometheus_range_matrix
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn parse_prometheus_range_matrix_extracts_series() {
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [
                    {
                        "metric": {"__name__": "up", "job": "node"},
                        "values": [[1_718_000_000.0, "1"], [1_718_000_060.0, "0"]]
                    }
                ]
            }
        });

        let series = parse_prometheus_range_matrix(&prom_json);
        assert_eq!(series.len(), 1);
        assert_eq!(
            series[0].labels.get("job").map(String::as_str),
            Some("node")
        );
        assert_eq!(series[0].points.len(), 2);
    }

    #[test]
    fn parse_prometheus_range_matrix_empty_result() {
        let prom_json = serde_json::json!({
            "status": "success",
            "data": { "resultType": "matrix", "result": [] }
        });
        let series = parse_prometheus_range_matrix(&prom_json);
        assert!(series.is_empty());
    }

    #[test]
    fn parse_prometheus_range_matrix_for_host_scopes_fleet_query() {
        let host = discovery_host();
        let host_id = host.host_id.to_string();
        let other_host_id = Uuid::new_v4().to_string();
        let prom_json = serde_json::json!({
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [
                    {
                        "metric": {
                            "piping_target_id": other_host_id,
                            "interface": "eth0"
                        },
                        "values": [[1_718_000_000.0, "0"]]
                    },
                    {
                        "metric": {
                            "piping_target_id": host_id,
                            "interface": "eth0"
                        },
                        "values": [[1_718_000_000.0, "1"]]
                    },
                    {
                        "metric": {
                            "piping_target_id": host_id,
                            "interface": "eth1"
                        },
                        "values": [[1_718_000_000.0, "1"]]
                    }
                ]
            }
        });
        let source = ResolvedMetricQuerySource {
            source_type: "prometheus".to_string(),
            query_url: "https://monitoring.example/api/v1/query".to_string(),
            token: None,
            info: LatestMetricSourceInfo {
                name: Some("Prometheus".to_string()),
                source_type: Some("prometheus".to_string()),
                configured_from: "monitoring_domain".to_string(),
                query_url: "https://monitoring.example/api/v1/query".to_string(),
                credential_ref: None,
            },
            timeout_ms: 5_000,
            options: serde_json::json!({}),
            pinned_resolution: None,
        };

        let series = parse_prometheus_range_matrix_for_host(prom_json, &source, &host, "up");

        assert_eq!(series.len(), 2);
        assert!(series.iter().all(|item| {
            item.labels.get("piping_target_id").map(String::as_str) == Some(host_id.as_str())
        }));
        assert_eq!(
            series
                .iter()
                .filter_map(|item| item.labels.get("interface").map(String::as_str))
                .collect::<Vec<_>>(),
            vec!["eth0", "eth1"]
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // parse_zabbix_history
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn parse_zabbix_history_maps_rows_to_single_series() {
        // Zabbix emits clock and value as strings.
        let history = serde_json::json!([
            { "itemid": "1001", "clock": "1718000000", "value": "0.5", "ns": "0" },
            { "itemid": "1001", "clock": "1718000060", "value": "0.7", "ns": "0" },
        ]);

        let host = discovery_host();
        let series = parse_zabbix_history(&history, &host, HISTORY_MAX_POINTS as usize)
            .expect("complete Zabbix history");

        assert_eq!(series.len(), 1);
        let series = &series[0];
        assert_eq!(series.points.len(), 2);
        // Same [timestamp, "value"] pair shape Prometheus yields: numeric clock,
        // string value.
        assert_eq!(
            series.points[0],
            serde_json::json!([1_718_000_000.0, "0.5"])
        );
        assert_eq!(
            series.points[1],
            serde_json::json!([1_718_000_060.0, "0.7"])
        );
        assert_eq!(
            series.labels.get("piping_host_id").map(String::as_str),
            Some(host.host_id.to_string().as_str())
        );
        assert_eq!(
            series.labels.get("piping_host_name").map(String::as_str),
            Some(host.host_name.as_str())
        );
    }

    #[test]
    fn parse_zabbix_history_rejects_non_array_response() {
        let host = discovery_host();
        let error = parse_zabbix_history(
            &serde_json::json!({ "error": "boom" }),
            &host,
            HISTORY_MAX_POINTS as usize,
        )
        .expect_err("malformed response must not look like empty history");
        assert_eq!(error, "Zabbix history response was not an array");
    }

    #[test]
    fn parse_zabbix_history_empty_array_yields_empty() {
        let host = discovery_host();
        let series =
            parse_zabbix_history(&serde_json::json!([]), &host, HISTORY_MAX_POINTS as usize)
                .expect("empty array is complete history");
        assert!(series.is_empty());
    }

    #[test]
    fn parse_zabbix_history_rejects_truncated_window() {
        let history = Value::Array(
            (0..9)
                .map(|clock| {
                    serde_json::json!({
                        "clock": clock.to_string(),
                        "value": clock.to_string(),
                    })
                })
                .collect(),
        );

        let error = parse_zabbix_history(&history, &discovery_host(), 2)
            .expect_err("fetch-budget sentinel must reveal truncation");

        assert_eq!(
            error,
            "Zabbix history exceeds the 8-sample fetch budget; the requested time window could not be returned completely"
        );
    }

    #[test]
    fn parse_zabbix_history_skips_malformed_rows_before_downsampling() {
        let history = serde_json::json!([
            { "clock": "bad", "value": "discard" },
            { "clock": "10", "value": 7 },
            { "clock": "20", "value": "keep-first" },
            { "clock": "30", "value": "keep-last" },
        ]);

        let series = parse_zabbix_history(&history, &discovery_host(), 2)
            .expect("malformed rows are skipped within a complete response");

        assert_eq!(series.len(), 1);
        assert_eq!(
            series[0].points,
            vec![
                serde_json::json!([20.0, "keep-first"]),
                serde_json::json!([30.0, "keep-last"]),
            ]
        );
    }

    #[test]
    fn downsample_points_keeps_first_and_last_within_budget() {
        let points: Vec<Value> = (0..10)
            .map(|i| serde_json::json!([f64::from(i), i.to_string()]))
            .collect();
        let sampled = downsample_points(points, 4);
        assert_eq!(sampled.len(), 4);
        assert_eq!(sampled[0], serde_json::json!([0.0, "0"]));
        assert_eq!(sampled[3], serde_json::json!([9.0, "9"]));
    }

    #[test]
    fn downsample_points_passthrough_when_within_budget() {
        let points: Vec<Value> = (0..3)
            .map(|i| serde_json::json!([f64::from(i), i.to_string()]))
            .collect();
        let sampled = downsample_points(points.clone(), 100);
        assert_eq!(sampled, points);
    }

    #[test]
    fn downsample_points_with_one_slot_keeps_the_first_sample() {
        let points: Vec<Value> = (0..3)
            .map(|i| serde_json::json!([f64::from(i), i.to_string()]))
            .collect();

        assert_eq!(
            downsample_points(points, 1),
            vec![serde_json::json!([0.0, "0"])]
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // Guardrail: step clamping
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn history_step_clamp_raises_step_to_fit_budget() {
        let end = HISTORY_MAX_POINTS as f64 * 60.0;
        let window = metric_history_window(0.0, end, Some(1)).expect("valid history window");

        assert_eq!(window.step, 60);
        assert_eq!(
            window.clamp_message.as_deref(),
            Some("Step was auto-raised from 1s to 60s to stay within the 11000-point budget")
        );
    }

    #[test]
    fn history_step_no_clamp_when_already_large_enough() {
        let window = metric_history_window(0.0, 3600.0, Some(60)).expect("valid history window");

        assert_eq!(window.step, 60);
        assert!(window.clamp_message.is_none());
    }

    #[test]
    fn history_window_rejects_equal_and_reversed_bounds() {
        for (start, end) in [(10.0, 10.0), (11.0, 10.0)] {
            let error = metric_history_window(start, end, None)
                .expect_err("non-increasing bounds must be rejected");
            assert!(matches!(
                error,
                AppError::Validation(message) if message == "`start` must be less than `end`"
            ));
        }
    }

    #[test]
    fn history_window_accepts_exact_max_range_and_rejects_one_second_more() {
        let boundary = metric_history_window(0.0, HISTORY_MAX_RANGE_SECS, None)
            .expect("exact 31-day boundary is valid");
        assert!(boundary.step >= HISTORY_MIN_STEP_SECS);

        let error = metric_history_window(0.0, HISTORY_MAX_RANGE_SECS + 1.0, None)
            .expect_err("range above the 31-day boundary must be rejected");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn history_window_rejects_zero_step() {
        let error =
            metric_history_window(0.0, 60.0, Some(0)).expect_err("zero step must be rejected");

        assert!(matches!(
            error,
            AppError::Validation(message) if message == "`step` must be at least 1s"
        ));
    }

    // --- Preset apply + quick-start helpers ---

    #[test]
    fn quick_start_slug_lowercases_and_collapses_separators() {
        assert_eq!(derive_quick_start_slug("My Prom!! Stack"), "my-prom-stack");
        assert_eq!(derive_quick_start_slug("___"), "quick-start");
        assert_eq!(derive_quick_start_slug("Already-slug"), "already-slug");
    }

    #[test]
    fn quick_start_slug_truncates_to_column_budget() {
        let long = "a".repeat(200);
        let slug = derive_quick_start_slug(&long);
        assert!(slug.len() <= 96, "slug must fit varchar(128) headroom");
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn quick_start_name_strips_scheme_and_keeps_host() {
        assert_eq!(
            derive_quick_start_name("https://vm.example/select/0/prometheus", "victoriametrics"),
            "victoriametrics vm.example"
        );
        assert_eq!(
            derive_quick_start_name("http://", "prometheus"),
            "prometheus quick-start"
        );
    }

    #[test]
    fn query_source_credentials_require_canonical_uuid_text_at_both_request_boundaries() {
        let credential_id =
            Uuid::parse_str("abcdefab-cdef-4abc-8def-abcdefabcdef").expect("valid UUID");
        let canonical = credential_id.to_string();
        let uppercase = canonical.to_ascii_uppercase();
        assert_ne!(
            uppercase, canonical,
            "fixture must exercise UUID case rejection"
        );
        let query_source_payload = |credential_ref: String| {
            serde_json::json!({
                "name": "VictoriaMetrics",
                "slug": "victoriametrics",
                "query_url": "https://vm.example/api/v1/query",
                "credential_ref": credential_ref,
            })
        };
        let quick_start_payload = |credential_ref: String| {
            serde_json::json!({
                "query_url": "https://vm.example/api/v1/query",
                "source_type": "victoriametrics",
                "credential_ref": credential_ref,
            })
        };

        let query_source: QuerySourceRequest =
            serde_json::from_value(query_source_payload(canonical.clone()))
                .expect("canonical query-source UUID must deserialize");
        assert_eq!(query_source.credential_ref, Some(credential_id));
        let quick_start: MonitoringQuickStartRequest =
            serde_json::from_value(quick_start_payload(canonical.clone()))
                .expect("canonical quick-start UUID must deserialize");
        assert_eq!(quick_start.credential_ref, Some(credential_id));

        for non_canonical in [
            credential_id.simple().to_string(),
            format!("{{{canonical}}}"),
            format!("urn:uuid:{canonical}"),
            uppercase,
        ] {
            assert!(
                serde_json::from_value::<QuerySourceRequest>(query_source_payload(
                    non_canonical.clone()
                ))
                .is_err(),
                "query-source API must reject non-canonical UUID `{non_canonical}`",
            );
            assert!(
                serde_json::from_value::<MonitoringQuickStartRequest>(quick_start_payload(
                    non_canonical.clone()
                ))
                .is_err(),
                "quick-start API must reject non-canonical UUID `{non_canonical}`",
            );
        }
    }

    #[test]
    fn preset_lookup_recognises_built_in_exporter_slugs() {
        for slug in ["node", "docker", "blackbox", "postgres"] {
            let preset = presets::find_preset_by_exporter_slug(slug)
                .expect("built-in preset exists for known exporter slugs");
            assert_eq!(preset.exporter_slug, slug);
            assert!(!preset.queries.is_empty(), "presets must carry queries");
            assert!(
                preset.set_slug.starts_with("preset-"),
                "preset set_slug must be name-spaced under `preset-`"
            );
        }
        assert!(presets::find_preset_by_exporter_slug("unknown").is_none());
    }

    #[test]
    fn preset_quick_start_request_round_trip_with_minimal_payload() {
        let payload: MonitoringQuickStartRequest = serde_json::from_value(serde_json::json!({
            "query_url": "https://vm.example/select/0/prometheus",
            "source_type": "prometheus",
        }))
        .expect("minimal quick-start payload deserialises");
        assert_eq!(payload.source_type, "prometheus");
        assert!(payload.scope_type.is_none());
        assert!(payload.preset_slug.is_none());
    }

    #[test]
    fn quick_start_slug_distinguishes_same_host_different_path() {
        // Folding source_type + URL into the slug avoids silently overwriting
        // an existing same-host source when the operator quick-starts a
        // different vmselect tenant or query endpoint on the same node.
        let a = derive_quick_start_slug(&format!(
            "{src} {url}",
            src = "victoriametrics",
            url = "https://vm.example/select/0/prometheus"
        ));
        let b = derive_quick_start_slug(&format!(
            "{src} {url}",
            src = "victoriametrics",
            url = "https://vm.example/select/1/prometheus"
        ));
        assert_ne!(
            a, b,
            "same host but different path must produce distinct slugs"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Display-config validator: rejects nonsense option payloads at
    // request boundary. Covers feature #4.
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn display_options_validator_accepts_empty_or_missing_display_block() {
        let empty = serde_json::json!({});
        assert!(validate_display_options("options", &empty).is_ok());
        let no_display = serde_json::json!({"unrelated": 1});
        assert!(validate_display_options("options", &no_display).is_ok());
        let empty_display = serde_json::json!({"display": {}});
        assert!(validate_display_options("options", &empty_display).is_ok());
    }

    #[test]
    fn display_options_validator_rejects_unknown_mode() {
        let options = serde_json::json!({
            "display": {"status": {"mode": "doom"}}
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(
            message.contains("options.display.status.mode must be one of"),
            "{message}"
        );
    }

    #[test]
    fn display_options_validator_rejects_numeric_mode_with_state_threshold() {
        let options = serde_json::json!({
            "display": {"status": {"mode": "numeric", "warning": ["down"], "critical": 90}}
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("warning must be a number"), "{message}");
    }

    #[test]
    fn display_options_validator_rejects_warning_greater_than_critical_higher_is_worse() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "numeric",
                    "direction": "higher_is_worse",
                    "warning": 95,
                    "critical": 90
                }
            }
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("warning"), "{message}");
        assert!(message.contains("critical"), "{message}");
    }

    #[test]
    fn display_options_validator_rejects_warning_less_than_critical_lower_is_worse() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "numeric",
                    "direction": "lower_is_worse",
                    "warning": 10,
                    "critical": 20
                }
            }
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("lower_is_worse"), "{message}");
    }

    #[test]
    fn display_options_validator_rejects_warning_equal_to_critical_higher_is_worse() {
        // Strict inequality: `warning == critical` is a no-trip rule that the
        // FE editor also rejects (thresholdValue.ts). Both validators must
        // agree, otherwise an operator can save a rule from one surface
        // (admin CLI / API) that the FE editor refuses to re-open.
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "numeric",
                    "direction": "higher_is_worse",
                    "warning": 90,
                    "critical": 90
                }
            }
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("warning"), "{message}");
        assert!(message.contains("critical"), "{message}");
    }

    #[test]
    fn display_options_validator_rejects_warning_equal_to_critical_lower_is_worse() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "numeric",
                    "direction": "lower_is_worse",
                    "warning": 20,
                    "critical": 20
                }
            }
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("lower_is_worse"), "{message}");
    }

    #[test]
    fn display_options_validator_accepts_valid_numeric_block() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "numeric",
                    "direction": "higher_is_worse",
                    "warning": 80,
                    "critical": 90
                }
            }
        });
        assert!(validate_display_options("options", &options).is_ok());
    }

    #[test]
    fn display_options_validator_rejects_duplicate_state_tokens() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "state",
                    "warning": ["pending", "Pending"],
                    "critical": ["down"]
                }
            }
        });
        let err = validate_display_options("options", &options).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("duplicate"), "{message}");
    }

    #[test]
    fn display_options_validator_accepts_state_block_with_distinct_tokens() {
        let options = serde_json::json!({
            "display": {
                "status": {
                    "mode": "state",
                    "warning": ["pending", "degraded"],
                    "critical": ["down", "error"],
                    "normal": ["up", "ok"]
                }
            }
        });
        assert!(validate_display_options("options", &options).is_ok());
    }

    #[test]
    fn validate_metric_query_rejects_payload_with_bad_display_block() {
        let mut payload = metric_query_payload("prometheus", "promql", "up");
        payload.options = Some(serde_json::json!({
            "display": {"status": {"mode": "numeric", "warning": 95, "critical": 90}}
        }));
        let err = validate_metric_query(payload).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    // ────────────────────────────────────────────────────────────────────
    // Host-override DTO validation. Covers feature #5 boundary.
    // ────────────────────────────────────────────────────────────────────

    fn host_override_payload(options: Option<Value>) -> MetricQueryHostOverrideRequest {
        MetricQueryHostOverrideRequest {
            metric_query_id: Uuid::new_v4(),
            host_id: Uuid::new_v4(),
            label_override: Some("custom label".to_string()),
            unit_override: Some("%".to_string()),
            options_override: options,
        }
    }

    #[test]
    fn host_override_validator_accepts_null_options() {
        let input = validate_host_override(host_override_payload(None)).unwrap();
        assert!(input.options_override.is_none());
        assert_eq!(input.label_override.as_deref(), Some("custom label"));
    }

    #[test]
    fn host_override_validator_rejects_non_object_options() {
        let err =
            validate_host_override(host_override_payload(Some(serde_json::json!(42)))).unwrap_err();
        let AppError::Validation(message) = err else {
            panic!("expected validation error");
        };
        assert!(message.contains("options_override"), "{message}");
    }

    #[test]
    fn host_override_validator_rejects_bad_threshold_ordering() {
        // options_override IS the display subtree, so `{status: {…}}`
        // becomes `{display: {status: {…}}}` internally.
        let bad = serde_json::json!({
            "status": {"mode": "numeric", "warning": 95, "critical": 90}
        });
        let err = validate_host_override(host_override_payload(Some(bad))).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[test]
    fn host_override_validator_accepts_valid_state_block() {
        let ok = serde_json::json!({
            "status": {"mode": "state", "critical": ["down"], "normal": ["up"]}
        });
        let input = validate_host_override(host_override_payload(Some(ok))).unwrap();
        assert!(input.options_override.is_some());
    }
}
