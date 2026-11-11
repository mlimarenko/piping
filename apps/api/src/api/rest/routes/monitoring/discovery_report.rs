//! Exporter discovery report ingestion route.
//!
//! The discovery playbook reports one host's bounded evidence with its sandbox
//! job token. This handler authenticates that run-bound token, resolves the
//! contract report against the host's own monitoring configuration, probes the
//! allowed endpoints, and stores the report as a run artifact. It never grants
//! catalog, assignment or source CRUD.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::api::access_control_helpers::build_access_control_service;
use crate::api::connection_authz_attributes::{
    batch_connection_target_is_allowed, resolve_connection_target_facets,
};
use crate::api::connection_target_visibility::{
    ConnectionTargetVisibility, resolve_job_bound_host_visibility,
};
use crate::api::middleware::error::handle_app_error;
use crate::api::middleware::request_id::RequestId;
use crate::api::rest::envelope_extractors::EnvelopeJson;
use crate::api::rest::request_context::build_request_context;
use crate::app::AppState;
use crate::contexts::access_control::domain::{Action, PolicyDecision, ResourceType};
use crate::contexts::inventory::domain::DiscoveryHostFilters;
use crate::contexts::inventory::domain::native::dataflow::EndpointType;
use crate::contexts::monitoring::application::exporter_discovery::DiscoveryCollectionArtifact;
use crate::contexts::monitoring::application::exporter_discovery_ingest::{
    DiscoveryIngestInput, ingest_discovery_run,
};
use crate::contexts::monitoring::application::exporter_discovery_reconcile::{
    DiscoveryPolicy, ReconcileOutcome, reconcile_report,
};
use crate::contexts::monitoring::application::exporter_probe::ProbeSettings;
use crate::contexts::monitoring::application::read_model::{
    enrich_hosts_with_monitoring, fetch_discovery_hosts,
};
use crate::contexts::monitoring::domain::exporter_discovery::{
    DISCOVERY_REPORT_ARTIFACT_TYPE, DiscoveryReport,
};
use crate::contexts::monitoring::infrastructure::discovery_store::HostScope;
use crate::contexts::playbooks::application::CreateArtifactInput;
use crate::contexts::playbooks::domain::JobTokenAudience;
use crate::contexts::shared::request_context::PlaybookJobBinding;
use crate::errors::AppError;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DiscoveryReportResponse {
    pub report: DiscoveryReport,
    pub artifact_id: Uuid,
    pub probe_target_count: usize,
    pub reconcile: ReconcileOutcome,
}

/// Ingest one host's exporter discovery evidence.
#[utoipa::path(
    post,
    path = "/api/v1/monitoring/discovery/report",
    tag = "monitoring",
    request_body = DiscoveryCollectionArtifact,
    responses(
        (status = 201, description = "Discovery report stored", body = DiscoveryReportResponse),
        (status = "4XX", description = "Discovery report rejected", body = crate::api::rest::error_mapping::ApiErrorResponse),
        (status = "5XX", description = "Discovery report ingestion failed", body = crate::api::rest::error_mapping::ApiErrorResponse),
    ),
)]
pub async fn ingest_discovery_report(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    EnvelopeJson(collection): EnvelopeJson<DiscoveryCollectionArtifact>,
) -> Response {
    let raw_token = match bearer_token(&headers) {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    let token = match state
        .services
        .playbooks
        .validate_artifact_job_token(raw_token.as_str(), Utc::now())
        .await
    {
        Ok(token) => token,
        Err(error) => return handle_app_error(error),
    };
    if let Err(error) = ensure_run_binding(&token, &collection) {
        return handle_app_error(error);
    }
    let run = match state.services.playbooks.get_run(token.run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return handle_app_error(AppError::Forbidden("discovery run not found".into())),
        Err(error) => return handle_app_error(error),
    };
    if run.run_group_id != collection.run_group_id {
        return handle_app_error(AppError::Forbidden(
            "discovery report group does not match the job token".to_string(),
        ));
    }
    if let Err(error) =
        authorize_reconciliation(&state, &request_id, &headers, &token, &collection).await
    {
        return handle_app_error(error);
    }

    let filters = DiscoveryHostFilters {
        host_id: Some(collection.host_id),
        fetch_limit: 1,
        ..DiscoveryHostFilters::default()
    };
    let hosts = match fetch_discovery_hosts(&state.repositories.discovery, &filters, None).await {
        Ok(hosts) => hosts,
        Err(error) => return handle_app_error(error),
    };
    let mut hosts =
        match enrich_hosts_with_monitoring(&state.db_pool, &state.settings.monitoring, hosts).await
        {
            Ok(hosts) => hosts,
            Err(error) => return handle_app_error(error),
        };
    let Some(host) = hosts.pop() else {
        return handle_app_error(AppError::NotFound(
            "Discovery host was not found".to_string(),
        ));
    };
    if host.kind != collection.source_kind {
        return handle_app_error(AppError::Validation(
            "discovery collection source kind does not match the host".to_string(),
        ));
    }

    let probe_settings = match ProbeSettings::validated(collection.options.probe_timeout_ms) {
        Ok(settings) => settings,
        Err(error) => return handle_app_error(error),
    };
    let policy = match DiscoveryPolicy::validated(
        &collection.options.absence_policy,
        collection.options.absence_confirm_runs,
        collection.options.restore_on_reappearance,
    ) {
        Ok(policy) => policy,
        Err(error) => return handle_app_error(error),
    };
    let host_name = host.host_name.clone();
    let source_kind = host.kind.clone();
    let host_scope = HostScope {
        site_id: host.site_id,
        tenant_id: host.tenant_id,
        role_id: host.role_id,
        kind: host.kind.clone(),
    };
    let result = match ingest_discovery_run(
        DiscoveryIngestInput {
            collection: collection.clone(),
            host,
        },
        probe_settings,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return handle_app_error(error),
    };
    let report_json = match serde_json::to_value(&result.report) {
        Ok(value) => value,
        Err(error) => {
            return handle_app_error(AppError::Internal(format!(
                "discovery report serialization failed: {error}"
            )));
        }
    };
    let artifact = match state
        .services
        .playbooks
        .create_artifact_with_job_token(
            collection.run_id,
            raw_token.as_str(),
            CreateArtifactInput {
                artifact_type: DISCOVERY_REPORT_ARTIFACT_TYPE.to_string(),
                name: format!("exporter-discovery-report-{}", collection.host_id),
                content_type: Some("application/json".to_string()),
                content_json: Some(report_json),
                content_text: None,
            },
            JobTokenAudience::Sandbox,
            Utc::now(),
        )
        .await
    {
        Ok(artifact) => artifact,
        Err(error) => return handle_app_error(error),
    };

    let reconcile = match reconcile_report(
        &state.db_pool,
        collection.host_id,
        &host_name,
        &source_kind,
        host_scope,
        policy,
        &result.report,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => return handle_app_error(error),
    };

    (
        StatusCode::CREATED,
        Json(DiscoveryReportResponse {
            report: result.report,
            artifact_id: artifact.id,
            probe_target_count: result.probe_target_count,
            reconcile,
        }),
    )
        .into_response()
}

async fn authorize_reconciliation(
    state: &AppState,
    request_id: &RequestId,
    headers: &HeaderMap,
    token: &crate::contexts::playbooks::domain::PlaybookJobToken,
    collection: &DiscoveryCollectionArtifact,
) -> Result<(), AppError> {
    let mut context = build_request_context(request_id, None, headers);
    context.playbook_job_binding = Some(PlaybookJobBinding {
        token_id: token.id,
        run_id: token.run_id,
        target_kind: token.target_kind.clone(),
        target_id: token.target_id,
    });
    let kind = match collection.source_kind.as_str() {
        "device" => EndpointType::Device,
        "virtual_machine" => EndpointType::VirtualMachine,
        _ => return Err(AppError::Forbidden("invalid discovery host kind".into())),
    };
    let visibility =
        resolve_job_bound_host_visibility(state, &context, kind, collection.host_id).await?;
    match visibility {
        ConnectionTargetVisibility::All => {}
        ConnectionTargetVisibility::None => {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        ConnectionTargetVisibility::Restricted(predicate) => {
            let facets = resolve_connection_target_facets(state, token.target_id)
                .await?
                .ok_or_else(|| AppError::Forbidden("discovery target unavailable".into()))?;
            if !batch_connection_target_is_allowed(&predicate, token.target_id, Some(&facets)) {
                return Err(AppError::Forbidden("Access denied".into()));
            }
        }
    }
    let group = state
        .services
        .playbooks
        .get_run_group_with_runs(collection.run_group_id, 1)
        .await?
        .ok_or_else(|| AppError::Forbidden("discovery run group unavailable".into()))?;
    let actor = crate::app::live_launch_principal(state, &group.run_group).await?;
    let service = build_access_control_service(state);
    for action in [Action::Create, Action::Update] {
        let decision = service
            .check_access_for_actor_in_request(
                &actor,
                &context,
                action,
                ResourceType::Monitoring,
                None,
                None,
            )
            .await?;
        if decision.decision != PolicyDecision::Allow {
            return Err(AppError::Forbidden("Access denied".into()));
        }
    }
    Ok(())
}

pub(crate) fn ensure_run_binding(
    token: &crate::contexts::playbooks::domain::PlaybookJobToken,
    collection: &DiscoveryCollectionArtifact,
) -> Result<(), AppError> {
    if token.run_id != collection.run_id {
        return Err(AppError::Forbidden(
            "discovery report run does not match the job token".to_string(),
        ));
    }
    if token.target_kind != "connection_target"
        || token.target_id != collection.connection_target_id
    {
        return Err(AppError::Forbidden(
            "discovery report target does not match the job token".to_string(),
        ));
    }
    if token.source_kind != collection.source_kind || token.source_id != collection.host_id {
        return Err(AppError::Forbidden(
            "discovery report source does not match the job token".to_string(),
        ));
    }
    Ok(())
}

fn bearer_token(headers: &HeaderMap) -> Result<String, AppError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Auth("TOKEN_MISSING".to_string()))?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AppError::Auth("TOKEN_INVALID".to_string()))?;
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::playbooks::domain::{
        JobTokenAudience, PlaybookJobToken, PlaybookJobTokenStatus,
    };
    use serde_json::json;

    fn token() -> PlaybookJobToken {
        PlaybookJobToken {
            id: Uuid::from_u128(1),
            run_id: Uuid::from_u128(2),
            target_kind: "connection_target".to_string(),
            target_id: Uuid::from_u128(3),
            source_kind: "virtual_machine".to_string(),
            source_id: Uuid::from_u128(4),
            token_prefix: "pjt".to_string(),
            status: PlaybookJobTokenStatus::Active,
            scopes: json!(["artifact:create"]),
            audience: JobTokenAudience::Sandbox,
            owner_user_id: None,
            issued_at: Utc::now(),
            expires_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
            metadata: json!({}),
        }
    }

    fn collection() -> DiscoveryCollectionArtifact {
        serde_json::from_value(json!({
            "schema_version": 1,
            "run_id": Uuid::from_u128(2),
            "run_group_id": Uuid::from_u128(5),
            "connection_target_id": Uuid::from_u128(3),
            "host_id": Uuid::from_u128(4),
            "host_name": "db-01",
            "source_kind": "virtual_machine",
            "platform": "linux",
            "started_at": "2026-11-10T00:00:00Z",
            "finished_at": "2026-11-10T00:00:05Z",
            "sources": {"host_inspection": true, "endpoint_probe": true},
            "coverage": {
                "listeners": "complete",
                "services": "complete",
                "processes": "complete",
                "containers": "not_installed",
                "listener_owners": "complete"
            },
            "raw": {}
        }))
        .unwrap()
    }

    #[test]
    fn run_binding_accepts_only_the_same_run_target_and_source() {
        let token = token();
        assert!(ensure_run_binding(&token, &collection()).is_ok());

        let mut wrong_run = collection();
        wrong_run.run_id = Uuid::from_u128(99);
        assert!(ensure_run_binding(&token, &wrong_run).is_err());

        let mut wrong_target = collection();
        wrong_target.connection_target_id = Uuid::from_u128(99);
        assert!(ensure_run_binding(&token, &wrong_target).is_err());

        let mut wrong_source = collection();
        wrong_source.host_id = Uuid::from_u128(99);
        assert!(ensure_run_binding(&token, &wrong_source).is_err());

        let mut wrong_kind = collection();
        wrong_kind.source_kind = "device".to_string();
        assert!(ensure_run_binding(&token, &wrong_kind).is_err());
    }
}
