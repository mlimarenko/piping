//! Application assembly: AppState and service wiring, router construction, server startup and background workers.
pub(crate) mod device_ssh_exec;
mod playbook_cancel_worker;
pub(crate) mod playbook_secret_gate;
mod worker_backoff;
mod workers;

// Narrow dispatch-time C3 test seam. `resolve_runtime_secret_env` is the real
// worker re-check (a `#[ignore]`d DB integration test drives it directly to
// prove a scheduled run's service principal is re-validated and its secret
// grant re-checked at dispatch); `SecretRecheckSubject` is its input. These are
// `pub` only so the external integration-test crate can reach the real worker
// function — no route, handler, utoipa schema or OpenAPI document references
// them, so the public transport contract is unchanged. `secret_recheck_subject`
// stays crate-private: its mapping test lives in `workers::tests`.
pub use workers::{SecretRecheckSubject, resolve_runtime_secret_env};

use std::error::Error;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{
        HeaderMap, StatusCode,
        header::{HeaderName, HeaderValue},
    },
    middleware,
    response::IntoResponse,
    routing::{delete, get, post},
};
use redis::aio::ConnectionManager;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::watch;
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

type ConnectionServiceType = ConnectionService<
    SqlxConnectionRepository,
    HttpProxyClient,
    AuditService<SqlxAuditRepository>,
    MaterializationService<SqlxConfigContextRepository>,
    SqlxSecretRepository,
    DbSecretStore,
>;
type UserCredentialSetServiceType = UserCredentialSetService<SqlxConnectionRepository>;

type InventoryAdminServiceType = InventoryAdminService<
    InventoryAdminRepositories<
        SqlxTenantGroupRepository,
        SqlxTenantRepository,
        SqlxSiteRepository,
        SqlxSiteGroupRepository,
        SqlxRegionRepository,
        SqlxClusterRepository,
        SqlxClusterTypeRepository,
        SqlxClusterGroupRepository,
        SqlxDeviceRoleRepository,
        SqlxDeviceRepository,
        SqlxManufacturerRepository,
        SqlxDeviceTypeRepository,
        SqlxVirtualMachineRepository,
    >,
>;

type SourceServiceType = SourceService<
    SqlxProviderRepository,
    SqlxSourceRuntimeRepository,
    SqlxOutboundEventRepository,
    SqlxSecretRepository,
    DbSecretStore,
>;
type WebhookServiceType = WebhookService<
    SqlxSourceRuntimeRepository,
    SqlxWebhookEventRepository,
    SqlxSiteRepository,
    AuditService<SqlxAuditRepository>,
>;
type OutboundServiceType = OutboundService<
    SqlxProviderRepository,
    SqlxOutboundEventRepository,
    SqlxSourceRuntimeRepository,
    SqlxSecretRepository,
    DbSecretStore,
>;
type AccessControlServiceType =
    AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;
type PolicyEvaluationServiceType =
    PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>;
type AccessControlQueryServiceType =
    AccessControlQueryService<SqlxRoleRepository, SqlxUserRepository, SqlxGroupRepository>;
type OidcSettingsServiceType =
    OidcSettingsService<SqlxOidcSettingsRepository, SqlxSecretRepository, DbSecretStore>;
type AssistantServiceType = AssistantService<SqlxAssistantRepository>;
type PlaybookServiceType = PlaybookService<SqlxPlaybookRepository>;
type SecretFieldIntentServiceType = SecretFieldIntentService<SqlxSecretRepository>;
type SecretFieldStateServiceType = SecretFieldStateService<SqlxSecretRepository, DbSecretStore>;
type CustomFieldDefinitionServiceType = CustomFieldDefinitionService<
    SqlxCustomFieldDefinitionRepository,
    SqlxCustomFieldChoiceSetRepository,
>;
type CustomFieldValueServiceType = CustomFieldValueService<
    SqlxCustomFieldDefinitionRepository,
    SqlxCustomFieldChoiceSetRepository,
    SqlxSecretRepository,
    DbSecretStore,
>;
type ConfigQueryServiceType = ConfigQueryService<SqlxConfigContextRepository, SqlxSecretRepository>;
type SyncQueryServiceType = SyncQueryService<SqlxProviderRepository, SqlxSyncJobRepository>;
type AccessControlAdminServiceType = AccessControlAdminService<
    SqlxUserRepository,
    SqlxUserCredentialRepository,
    SqlxGroupRepository,
    SqlxRoleRepository,
    SqlxRoleAssignmentRepository,
    SqlxPolicyRepository,
    SecretFieldIntentService<SqlxSecretRepository>,
>;
type WizardServiceType = WizardService;

use crate::config::metrics_scrape_disabled_warning;
use crate::contexts::access_control::application::admin_service::AccessControlAdminDependencies;
use crate::{
    api::{
        access_control_helpers::{
            build_access_control_service_with_cache_from_repos,
            build_policy_evaluation_service_with_cache_from_repos,
        },
        middleware::{
            auth::auth_middleware, cors::build_cors_layer, error::error_middleware,
            metrics::metrics_middleware, request_id::request_id_middleware,
            tracing::tracing_middleware,
        },
        rest::router::{RestRouteLimits, build_rest_router_with_limits},
    },
    config,
    contexts::{
        access_control::application::{
            AccessControlAdminService, AccessControlQueryService, AccessControlService,
            AuthService, OidcSettingsService, PolicyEvaluationService, SettingsService,
            UserApiTokenService,
        },
        access_control::infrastructure::persistence::{
            SqlxAuthSessionRepository, SqlxGroupRepository, SqlxOidcSettingsRepository,
            SqlxPolicyRepository, SqlxRoleAssignmentRepository, SqlxRoleRepository,
            SqlxSettingsRepository, SqlxUserApiTokenRepository, SqlxUserCredentialRepository,
            SqlxUserRepository,
        },
        assistant::{
            application::AssistantService, infrastructure::persistence::SqlxAssistantRepository,
        },
        audit::application::{AuditQueryService, AuditService},
        audit::infrastructure::persistence::SqlxAuditRepository,
        config::infrastructure::persistence::{
            SqlxInstanceBrandingRepository, SqlxMonitoringSdLabelSettingsRepository,
        },
        connection_config::{SqlxConnConfAssignmentRepository, SqlxConnConfRecordRepository},
        connections::application::connection_service::ConnectionServiceDeps,
        connections::application::{
            ConnectionService, SessionRelayHealthRegistry, SharedSessionRelayHealthRegistry,
            UserCredentialSetService,
        },
        connections::infrastructure::persistence::{
            SqlxConnectionFilterPresetRepository, SqlxConnectionRepository,
        },
        custom_fields::application::{CustomFieldDefinitionService, CustomFieldValueService},
        custom_fields::infrastructure::{
            SqlxCustomFieldChoiceSetRepository, SqlxCustomFieldDefinitionRepository,
        },
        inventory::application::{
            BackupService, BulkResourceService, ConfigQueryService, InventoryAdminRepositories,
            InventoryAdminService, IpamService, LayoutService, MaterializationService,
            TemplateService,
        },
        inventory::infrastructure::caching::HostConfigCache,
        inventory::infrastructure::persistence::{
            SqlxAggregateRepository, SqlxAsnRangeRepository, SqlxAsnRepository,
            SqlxBulkResourceRepository, SqlxCableRepository, SqlxCablingRepository,
            SqlxCircuitRepository, SqlxCircuitTypeRepository, SqlxClusterGroupRepository,
            SqlxClusterRepository, SqlxClusterTypeRepository, SqlxCollectedFactsRepository,
            SqlxConfigContextRepository, SqlxConnectionTargetRepository, SqlxConsolePortRepository,
            SqlxConsoleServerPortRepository, SqlxContactAssignmentRepository,
            SqlxContactGroupRepository, SqlxContactRepository, SqlxContactRoleRepository,
            SqlxDeviceBayRepository, SqlxDeviceRepository, SqlxDeviceRoleRepository,
            SqlxDeviceTypeRepository, SqlxDiscoveryRepository, SqlxFrontPortRepository,
            SqlxInterfaceRepository, SqlxInventoryItemRepository, SqlxInventoryItemRoleRepository,
            SqlxIpAddressRepository, SqlxIpRangeRepository, SqlxIpamRoleRepository,
            SqlxLayoutRepository, SqlxLocationRepository, SqlxManufacturerRepository,
            SqlxModuleBayRepository, SqlxModuleRepository, SqlxModuleTypeProfileRepository,
            SqlxModuleTypeRepository, SqlxObjectRefResolver, SqlxPlatformRepository,
            SqlxPowerOutletRepository, SqlxPowerPortRepository, SqlxPrefixRepository,
            SqlxProviderNetworkRepository,
            SqlxProviderRepository as SqlxInventoryProviderRepository, SqlxRackRepository,
            SqlxRackRoleRepository, SqlxRackTypeRepository, SqlxRearPortRepository,
            SqlxRegionRepository, SqlxRirRepository, SqlxRouteTargetRepository,
            SqlxServiceRepository, SqlxSiteGroupRepository, SqlxSiteRepository,
            SqlxTemplateRepository, SqlxTemplateStepResourceRepository, SqlxTenantGroupRepository,
            SqlxTenantRepository, SqlxVirtualChassisRepository, SqlxVirtualDiskRepository,
            SqlxVirtualMachineRepository, SqlxVlanGroupRepository, SqlxVlanRepository,
            SqlxVmInterfaceRepository, SqlxVpnIkePolicyRepository, SqlxVpnIkeProposalRepository,
            SqlxVpnIpsecPolicyRepository, SqlxVpnIpsecProfileRepository,
            SqlxVpnIpsecProposalRepository, SqlxVpnTunnelGroupRepository, SqlxVpnTunnelRepository,
            SqlxVpnTunnelTerminationRepository, SqlxVrfRepository,
        },
        inventory::infrastructure::persistence::{
            SqlxCircuitGroupRepository, SqlxCircuitTerminationRepository, SqlxMacAddressRepository,
            SqlxPowerFeedRepository, SqlxPowerPanelRepository, SqlxProviderAccountRepository,
            SqlxRackReservationRepository, SqlxServiceTemplateRepository,
            SqlxVirtualCircuitRepository, SqlxVirtualCircuitTypeRepository,
            SqlxVirtualDeviceContextRepository, SqlxVirtualMachineTypeRepository,
        },
        licensing::application::{
            activation_service::LicenseActivationService,
            login_gate_service::LicenseLoginGateService, service::LicensingService,
            status_service::LicenseStatusService,
        },
        playbooks::{
            application::{PlaybookService, RunEventBroadcaster},
            infrastructure::{official_catalog, persistence::SqlxPlaybookRepository},
        },
        secrets::application::{SecretFieldIntentService, SecretFieldStateService},
        secrets::infrastructure::persistence::{DbSecretStore, SqlxSecretRepository},
        setup::{
            application::WizardService,
            infrastructure::persistence::wizard_progress_repository::WizardProgressRepository,
        },
        sync::application::outbound_service::OutboundWorkerConfig,
        sync::application::source_service::SourceRuntimeConfig,
        sync::application::{OutboundService, SourceService, SyncQueryService, WebhookService},
        sync::infrastructure::persistence::{
            SqlxMappingConfigurationRepository, SqlxOutboundEventRepository,
            SqlxProviderRepository, SqlxProviderRequestJournalRepository,
            SqlxSourceBootstrapStateRepository, SqlxSourceRuntimeRepository, SqlxSyncJobRepository,
            SqlxWebhookEventRepository,
        },
    },
    errors::AppError,
    integrations::proxy::{
        HttpProxyClient, ProxySharedSecretProvider, StaticProxySharedSecretProvider,
    },
    observability::{EventFields, LogLevel, log_error_event, log_event},
    proxy,
};

const PROXY_DISABLED_MESSAGE: &str = "Proxy routes are disabled";
const PROXY_WS_PROTOCOL_GUACAMOLE: [&str; 1] = ["guacamole"];
const PROXY_WS_PROTOCOL_VNC: [&str; 2] = ["binary", "base64"];
#[derive(Clone)]
pub struct AppRepositories {
    pub owners: crate::contexts::inventory::infrastructure::persistence::SqlxOwnerRepository,
    pub owner_groups: crate::contexts::inventory::infrastructure::persistence::SqlxOwnerGroupRepository,
    pub users: SqlxUserRepository,
    pub user_credentials: SqlxUserCredentialRepository,
    pub auth_sessions: SqlxAuthSessionRepository,
    pub user_api_tokens: SqlxUserApiTokenRepository,
    pub settings: SqlxSettingsRepository,
    pub assistant: SqlxAssistantRepository,
    pub playbooks: SqlxPlaybookRepository,
    pub oidc_settings: SqlxOidcSettingsRepository,
    pub groups: SqlxGroupRepository,
    pub roles: SqlxRoleRepository,
    pub role_assignments: SqlxRoleAssignmentRepository,
    pub policies: SqlxPolicyRepository,
    pub connections: SqlxConnectionRepository,
    pub connection_filter_presets: SqlxConnectionFilterPresetRepository,
    pub tenant_groups: SqlxTenantGroupRepository,
    pub tenants: SqlxTenantRepository,
    pub sites: SqlxSiteRepository,
    pub site_groups: SqlxSiteGroupRepository,
    pub regions: SqlxRegionRepository,
    pub clusters: SqlxClusterRepository,
    pub cluster_types: SqlxClusterTypeRepository,
    pub cluster_groups: SqlxClusterGroupRepository,
    pub device_roles: SqlxDeviceRoleRepository,
    pub devices: SqlxDeviceRepository,
    pub manufacturers: SqlxManufacturerRepository,
    pub device_types: SqlxDeviceTypeRepository,
    pub virtual_machines: SqlxVirtualMachineRepository,
    pub virtual_machine_types: SqlxVirtualMachineTypeRepository,
    pub discovery: SqlxDiscoveryRepository,
    pub connection_targets: SqlxConnectionTargetRepository,
    pub collected_facts: SqlxCollectedFactsRepository,
    pub interfaces: SqlxInterfaceRepository,
    pub ip_addresses: SqlxIpAddressRepository,
    pub ipam_roles: SqlxIpamRoleRepository,
    pub vlans: SqlxVlanRepository,
    pub vlan_groups: SqlxVlanGroupRepository,
    pub racks: SqlxRackRepository,
    pub rack_roles: SqlxRackRoleRepository,
    pub rack_types: SqlxRackTypeRepository,
    pub rirs: SqlxRirRepository,
    pub route_targets: SqlxRouteTargetRepository,
    pub aggregates: SqlxAggregateRepository,
    pub asns: SqlxAsnRepository,
    pub ip_ranges: SqlxIpRangeRepository,
    pub services: SqlxServiceRepository,
    pub dataflows: crate::contexts::inventory::infrastructure::persistence::repositories::native::dataflow_repository::SqlxDataflowRepository,
    pub virtual_disks: SqlxVirtualDiskRepository,
    pub console_ports: SqlxConsolePortRepository,
    pub console_server_ports: SqlxConsoleServerPortRepository,
    pub front_ports: SqlxFrontPortRepository,
    pub inventory_items: SqlxInventoryItemRepository,
    pub modules: SqlxModuleRepository,
    pub module_types: SqlxModuleTypeRepository,
    pub module_type_profiles: SqlxModuleTypeProfileRepository,
    pub tags: crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::SqlxTagRepository,
    pub circuit_types: SqlxCircuitTypeRepository,
    pub circuit_groups: SqlxCircuitGroupRepository,
    pub provider_accounts: SqlxProviderAccountRepository,
    pub circuit_terminations: SqlxCircuitTerminationRepository,
    pub virtual_circuit_types: SqlxVirtualCircuitTypeRepository,
    pub virtual_circuits: SqlxVirtualCircuitRepository,
    pub power_panels: SqlxPowerPanelRepository,
    pub power_feeds: SqlxPowerFeedRepository,
    pub rack_reservations: SqlxRackReservationRepository,
    pub mac_addresses: SqlxMacAddressRepository,
    pub virtual_device_contexts: SqlxVirtualDeviceContextRepository,
    pub service_templates: SqlxServiceTemplateRepository,
    pub vpn_tunnel_groups: SqlxVpnTunnelGroupRepository,
    pub vpn_ike_policies: SqlxVpnIkePolicyRepository,
    pub vpn_ike_proposals: SqlxVpnIkeProposalRepository,
    pub vpn_ipsec_policies: SqlxVpnIpsecPolicyRepository,
    pub vpn_ipsec_proposals: SqlxVpnIpsecProposalRepository,
    pub vpn_ipsec_profiles: SqlxVpnIpsecProfileRepository,
    pub vpn_tunnels: SqlxVpnTunnelRepository,
    pub vpn_tunnel_terminations: SqlxVpnTunnelTerminationRepository,
    pub circuits: SqlxCircuitRepository,
    pub asn_ranges: SqlxAsnRangeRepository,
    pub providers: SqlxInventoryProviderRepository,
    pub inventory_item_roles: SqlxInventoryItemRoleRepository,
    pub provider_networks: SqlxProviderNetworkRepository,
    pub power_outlets: SqlxPowerOutletRepository,
    pub virtual_chassis: SqlxVirtualChassisRepository,
    pub device_bays: SqlxDeviceBayRepository,
    pub module_bays: SqlxModuleBayRepository,
    pub power_ports: SqlxPowerPortRepository,
    pub rear_ports: SqlxRearPortRepository,
    pub vrfs: SqlxVrfRepository,
    pub locations: SqlxLocationRepository,
    pub cabling: SqlxCablingRepository,
    pub cables: SqlxCableRepository,
    pub platforms: SqlxPlatformRepository,
    pub prefixes: SqlxPrefixRepository,
    pub vm_interfaces: SqlxVmInterfaceRepository,
    pub contacts: SqlxContactRepository,
    pub contact_groups: SqlxContactGroupRepository,
    pub contact_roles: SqlxContactRoleRepository,
    pub contact_assignments: SqlxContactAssignmentRepository,
    pub object_ref_resolver: SqlxObjectRefResolver,
    pub config_contexts: SqlxConfigContextRepository,
    pub secrets: SqlxSecretRepository,
    pub audit: SqlxAuditRepository,
    pub sync_providers: SqlxProviderRepository,
    pub sync_mapping_configurations: SqlxMappingConfigurationRepository,
    pub sync_jobs: SqlxSyncJobRepository,
    pub sync_source_runtime: SqlxSourceRuntimeRepository,
    pub sync_webhook_events: SqlxWebhookEventRepository,
    pub sync_outbound_events: SqlxOutboundEventRepository,
    pub sync_provider_request_journal: SqlxProviderRequestJournalRepository,
    pub sync_source_bootstrap_states: SqlxSourceBootstrapStateRepository,
    pub templates: SqlxTemplateRepository,
    pub template_step_resources: SqlxTemplateStepResourceRepository,
    pub layouts: SqlxLayoutRepository,
    pub bulk_resources: SqlxBulkResourceRepository,
    pub connconf_records: SqlxConnConfRecordRepository,
    pub connconf_assignments: SqlxConnConfAssignmentRepository,
    pub custom_field_definitions: SqlxCustomFieldDefinitionRepository,
    pub inventory_custom_field_data: crate::contexts::inventory::infrastructure::persistence::repositories::custom_field_data_repository::SqlxCustomFieldDataRepository,
    pub custom_field_choice_sets: SqlxCustomFieldChoiceSetRepository,
    pub instance_branding: SqlxInstanceBrandingRepository,
    pub monitoring_sd_label_settings: SqlxMonitoringSdLabelSettingsRepository,
}

impl AppRepositories {
    pub fn new(db_pool: &PgPool) -> Self {
        Self {
            owners: crate::contexts::inventory::infrastructure::persistence::SqlxOwnerRepository::new(db_pool.clone()),
            owner_groups: crate::contexts::inventory::infrastructure::persistence::SqlxOwnerGroupRepository::new(db_pool.clone()),
            users: SqlxUserRepository::new(db_pool.clone()),
            user_credentials: SqlxUserCredentialRepository::new(db_pool.clone()),
            auth_sessions: SqlxAuthSessionRepository::new(db_pool.clone()),
            user_api_tokens: SqlxUserApiTokenRepository::new(db_pool.clone()),
            settings: SqlxSettingsRepository::new(db_pool.clone()),
            assistant: SqlxAssistantRepository::new(db_pool.clone()),
            playbooks: SqlxPlaybookRepository::new(db_pool.clone()),
            oidc_settings: SqlxOidcSettingsRepository::new(db_pool.clone()),
            groups: SqlxGroupRepository::new(db_pool.clone()),
            roles: SqlxRoleRepository::new(db_pool.clone()),
            role_assignments: SqlxRoleAssignmentRepository::new(db_pool.clone()),
            policies: SqlxPolicyRepository::new(db_pool.clone()),
            connections: SqlxConnectionRepository::new(db_pool.clone()),
            connection_filter_presets: SqlxConnectionFilterPresetRepository::new(db_pool.clone()),
            tenant_groups: SqlxTenantGroupRepository::new(db_pool.clone()),
            tenants: SqlxTenantRepository::new(db_pool.clone()),
            sites: SqlxSiteRepository::new(db_pool.clone()),
            site_groups: SqlxSiteGroupRepository::new(db_pool.clone()),
            regions: SqlxRegionRepository::new(db_pool.clone()),
            clusters: SqlxClusterRepository::new(db_pool.clone()),
            cluster_types: SqlxClusterTypeRepository::new(db_pool.clone()),
            cluster_groups: SqlxClusterGroupRepository::new(db_pool.clone()),
            device_roles: SqlxDeviceRoleRepository::new(db_pool.clone()),
            devices: SqlxDeviceRepository::new(db_pool.clone()),
            manufacturers: SqlxManufacturerRepository::new(db_pool.clone()),
            device_types: SqlxDeviceTypeRepository::new(db_pool.clone()),
            virtual_machines: SqlxVirtualMachineRepository::new(db_pool.clone()),
            virtual_machine_types: SqlxVirtualMachineTypeRepository::new(db_pool.clone()),
            discovery: SqlxDiscoveryRepository::new(db_pool.clone()),
            connection_targets: SqlxConnectionTargetRepository::new(db_pool.clone()),
            collected_facts: SqlxCollectedFactsRepository::new(db_pool.clone()),
            interfaces: SqlxInterfaceRepository::new(db_pool.clone()),
            ip_addresses: SqlxIpAddressRepository::new(db_pool.clone()),
            ipam_roles: SqlxIpamRoleRepository::new(db_pool.clone()),
            vlans: SqlxVlanRepository::new(db_pool.clone()),
            vlan_groups: SqlxVlanGroupRepository::new(db_pool.clone()),
            racks: SqlxRackRepository::new(db_pool.clone()),
            rack_roles: SqlxRackRoleRepository::new(db_pool.clone()),
            rack_types: SqlxRackTypeRepository::new(db_pool.clone()),
            rirs: SqlxRirRepository::new(db_pool.clone()),
            route_targets: SqlxRouteTargetRepository::new(db_pool.clone()),
            aggregates: SqlxAggregateRepository::new(db_pool.clone()),
            asns: SqlxAsnRepository::new(db_pool.clone()),
            ip_ranges: SqlxIpRangeRepository::new(db_pool.clone()),
            services: SqlxServiceRepository::new(db_pool.clone()),
            dataflows: crate::contexts::inventory::infrastructure::persistence::repositories::native::dataflow_repository::SqlxDataflowRepository::new(db_pool.clone()),
            virtual_disks: SqlxVirtualDiskRepository::new(db_pool.clone()),
            console_ports: SqlxConsolePortRepository::new(db_pool.clone()),
            console_server_ports: SqlxConsoleServerPortRepository::new(db_pool.clone()),
            front_ports: SqlxFrontPortRepository::new(db_pool.clone()),
            inventory_items: SqlxInventoryItemRepository::new(db_pool.clone()),
            modules: SqlxModuleRepository::new(db_pool.clone()),
            module_types: SqlxModuleTypeRepository::new(db_pool.clone()),
            module_type_profiles: SqlxModuleTypeProfileRepository::new(db_pool.clone()),
            tags: crate::contexts::inventory::infrastructure::persistence::repositories::extras::tag_repository::SqlxTagRepository::new(db_pool.clone()),
            circuit_types: SqlxCircuitTypeRepository::new(db_pool.clone()),
            circuit_groups: SqlxCircuitGroupRepository::new(db_pool.clone()),
            provider_accounts: SqlxProviderAccountRepository::new(db_pool.clone()),
            circuit_terminations: SqlxCircuitTerminationRepository::new(db_pool.clone()),
            virtual_circuit_types: SqlxVirtualCircuitTypeRepository::new(db_pool.clone()),
            virtual_circuits: SqlxVirtualCircuitRepository::new(db_pool.clone()),
            power_panels: SqlxPowerPanelRepository::new(db_pool.clone()),
            power_feeds: SqlxPowerFeedRepository::new(db_pool.clone()),
            rack_reservations: SqlxRackReservationRepository::new(db_pool.clone()),
            mac_addresses: SqlxMacAddressRepository::new(db_pool.clone()),
            virtual_device_contexts: SqlxVirtualDeviceContextRepository::new(db_pool.clone()),
            service_templates: SqlxServiceTemplateRepository::new(db_pool.clone()),
            vpn_tunnel_groups: SqlxVpnTunnelGroupRepository::new(db_pool.clone()),
            vpn_ike_policies: SqlxVpnIkePolicyRepository::new(db_pool.clone()),
            vpn_ike_proposals: SqlxVpnIkeProposalRepository::new(db_pool.clone()),
            vpn_ipsec_policies: SqlxVpnIpsecPolicyRepository::new(db_pool.clone()),
            vpn_ipsec_proposals: SqlxVpnIpsecProposalRepository::new(db_pool.clone()),
            vpn_ipsec_profiles: SqlxVpnIpsecProfileRepository::new(db_pool.clone()),
            vpn_tunnels: SqlxVpnTunnelRepository::new(db_pool.clone()),
            vpn_tunnel_terminations: SqlxVpnTunnelTerminationRepository::new(db_pool.clone()),
            circuits: SqlxCircuitRepository::new(db_pool.clone()),
            asn_ranges: SqlxAsnRangeRepository::new(db_pool.clone()),
            providers: SqlxInventoryProviderRepository::new(db_pool.clone()),
            inventory_item_roles: SqlxInventoryItemRoleRepository::new(db_pool.clone()),
            provider_networks: SqlxProviderNetworkRepository::new(db_pool.clone()),
            power_outlets: SqlxPowerOutletRepository::new(db_pool.clone()),
            virtual_chassis: SqlxVirtualChassisRepository::new(db_pool.clone()),
            device_bays: SqlxDeviceBayRepository::new(db_pool.clone()),
            module_bays: SqlxModuleBayRepository::new(db_pool.clone()),
            power_ports: SqlxPowerPortRepository::new(db_pool.clone()),
            rear_ports: SqlxRearPortRepository::new(db_pool.clone()),
            vrfs: SqlxVrfRepository::new(db_pool.clone()),
            locations: SqlxLocationRepository::new(db_pool.clone()),
            cabling: SqlxCablingRepository::new(db_pool.clone()),
            cables: SqlxCableRepository::new(db_pool.clone()),
            platforms: SqlxPlatformRepository::new(db_pool.clone()),
            prefixes: SqlxPrefixRepository::new(db_pool.clone()),
            vm_interfaces: SqlxVmInterfaceRepository::new(db_pool.clone()),
            contacts: SqlxContactRepository::new(db_pool.clone()),
            contact_groups: SqlxContactGroupRepository::new(db_pool.clone()),
            contact_roles: SqlxContactRoleRepository::new(db_pool.clone()),
            contact_assignments: SqlxContactAssignmentRepository::new(db_pool.clone()),
            object_ref_resolver: SqlxObjectRefResolver::new(db_pool.clone()),
            config_contexts: SqlxConfigContextRepository::new(db_pool.clone()),
            secrets: SqlxSecretRepository::new(db_pool.clone()),
            audit: SqlxAuditRepository::new(db_pool.clone()),
            sync_providers: SqlxProviderRepository::new(db_pool.clone()),
            sync_mapping_configurations: SqlxMappingConfigurationRepository::new(db_pool.clone()),
            sync_jobs: SqlxSyncJobRepository::new(db_pool.clone()),
            sync_source_runtime: SqlxSourceRuntimeRepository::new(db_pool.clone()),
            sync_webhook_events: SqlxWebhookEventRepository::new(db_pool.clone()),
            sync_outbound_events: SqlxOutboundEventRepository::new(db_pool.clone()),
            sync_provider_request_journal: SqlxProviderRequestJournalRepository::new(
                db_pool.clone(),
            ),
            sync_source_bootstrap_states: SqlxSourceBootstrapStateRepository::new(db_pool.clone()),
            templates: SqlxTemplateRepository::new(db_pool.clone()),
            template_step_resources: SqlxTemplateStepResourceRepository::new(),
            layouts: SqlxLayoutRepository::new(db_pool.clone()),
            bulk_resources: SqlxBulkResourceRepository::new(db_pool.clone()),
            connconf_records: SqlxConnConfRecordRepository::new(db_pool.clone()),
            connconf_assignments: SqlxConnConfAssignmentRepository::new(db_pool.clone()),
            custom_field_definitions: SqlxCustomFieldDefinitionRepository::new(db_pool.clone()),
            inventory_custom_field_data: crate::contexts::inventory::infrastructure::persistence::repositories::custom_field_data_repository::SqlxCustomFieldDataRepository::new(db_pool.clone()),
            custom_field_choice_sets: SqlxCustomFieldChoiceSetRepository::new(db_pool.clone()),
            instance_branding: SqlxInstanceBrandingRepository::new(db_pool.clone()),
            monitoring_sd_label_settings: SqlxMonitoringSdLabelSettingsRepository::new(
                db_pool.clone(),
            ),
        }
    }
}

#[derive(Clone)]
pub struct AppServices {
    pub access_control: Arc<AccessControlServiceType>,
    pub policy_evaluation: Arc<PolicyEvaluationServiceType>,
    pub auth: Arc<AuthService>,
    pub user_api_tokens: Arc<UserApiTokenService>,
    pub settings: Arc<SettingsService>,
    pub assistant: Arc<AssistantServiceType>,
    pub playbooks: Arc<PlaybookServiceType>,
    pub oidc_settings: Arc<OidcSettingsServiceType>,
    pub materializer: Arc<MaterializationService<SqlxConfigContextRepository>>,
    pub secret_store: Arc<DbSecretStore>,
    pub secret_field_intents: Arc<SecretFieldIntentServiceType>,
    pub secret_field_states: Arc<SecretFieldStateServiceType>,
    pub custom_field_definitions: Arc<CustomFieldDefinitionServiceType>,
    pub custom_field_values: Arc<CustomFieldValueServiceType>,
    /// Choice-set CRUD is plain persistence with no orchestration, so the
    /// repository is the service surface directly.
    pub custom_field_choice_sets: Arc<SqlxCustomFieldChoiceSetRepository>,
    pub proxy_shared_secret_provider: Arc<dyn ProxySharedSecretProvider>,
    pub connection: Arc<ConnectionServiceType>,
    pub user_credential_sets: Arc<UserCredentialSetServiceType>,
    pub wizard: Arc<WizardServiceType>,
    pub audit: Arc<AuditService<SqlxAuditRepository>>,
    pub audit_query: Arc<AuditQueryService<SqlxAuditRepository>>,
    pub access_control_admin: Arc<AccessControlAdminServiceType>,
    pub access_control_queries: Arc<AccessControlQueryServiceType>,
    pub inventory_admin: Arc<InventoryAdminServiceType>,
    pub backup: Arc<BackupService>,
    pub ipam: Arc<IpamService>,
    pub config_queries: Arc<ConfigQueryServiceType>,
    pub sync_queries: Arc<SyncQueryServiceType>,
    pub source_service: Arc<SourceServiceType>,
    pub webhook_service: Arc<WebhookServiceType>,
    pub outbound_service: Arc<OutboundServiceType>,
    pub licensing: Arc<LicensingService>,
    pub license_activation: Arc<LicenseActivationService>,
    pub license_status: Arc<LicenseStatusService>,
    pub license_login_gate: Arc<LicenseLoginGateService>,
    pub inventory_templates: Arc<TemplateService>,
    pub inventory_layouts: Arc<LayoutService>,
    pub inventory_bulk_resources: Arc<BulkResourceService>,
    /// Per-process cache for `GET
    /// /api/v1/providers/{id}/sample-record?model=...`. Read on every preview
    /// keystroke; invalidated by `provider_registry/refresh`.
    pub sample_record_cache: Arc<crate::contexts::sync::application::SampleRecordCache>,
    pub webhook_subscriptions: Arc<crate::contexts::sync::application::WebhookSubscriptionService>,
}

mod boot {
    use super::*;

    pub(super) struct SecretServices {
        pub secret_store: Arc<DbSecretStore>,
        pub oidc_settings: Arc<OidcSettingsServiceType>,
        pub secret_field_intents: Arc<SecretFieldIntentServiceType>,
        pub secret_field_states: Arc<SecretFieldStateServiceType>,
    }

    pub(super) struct PolicyServices {
        pub access_control: Arc<AccessControlServiceType>,
        pub policy_evaluation: Arc<PolicyEvaluationServiceType>,
        pub access_control_admin: Arc<AccessControlAdminServiceType>,
        pub access_control_queries: Arc<AccessControlQueryServiceType>,
    }

    pub(super) struct InventoryAndSyncServices {
        pub inventory_admin: Arc<InventoryAdminServiceType>,
        pub backup: Arc<BackupService>,
        pub ipam: Arc<IpamService>,
        pub config_queries: Arc<ConfigQueryServiceType>,
        pub sync_queries: Arc<SyncQueryServiceType>,
        pub source_service: Arc<SourceServiceType>,
        pub webhook_service: Arc<WebhookServiceType>,
        pub outbound_service: Arc<OutboundServiceType>,
    }

    pub(super) struct LicenseServices {
        pub licensing: Arc<LicensingService>,
        pub license_activation: Arc<LicenseActivationService>,
        pub license_status: Arc<LicenseStatusService>,
        pub license_login_gate: Arc<LicenseLoginGateService>,
    }

    pub(super) fn build_secret_services(
        repositories: &AppRepositories,
        db_pool: &PgPool,
    ) -> SecretServices {
        let secret_store = Arc::new(DbSecretStore::new(db_pool.clone()));
        let oidc_settings = Arc::new(OidcSettingsService::new(
            Arc::new(repositories.oidc_settings.clone()),
            Arc::new(repositories.secrets.clone()),
            secret_store.clone(),
        ));
        let secret_field_intents = Arc::new(SecretFieldIntentService::new(Arc::new(
            repositories.secrets.clone(),
        )));
        let secret_field_states = Arc::new(SecretFieldStateService::new(
            Arc::new(repositories.secrets.clone()),
            secret_store.clone(),
        ));

        SecretServices {
            secret_store,
            oidc_settings,
            secret_field_intents,
            secret_field_states,
        }
    }

    pub(super) fn build_policy_services(
        repositories: &AppRepositories,
        settings: &config::Settings,
        redis: ConnectionManager,
        secret_field_intents: Arc<SecretFieldIntentServiceType>,
    ) -> PolicyServices {
        // The evaluation path takes the manager unwrapped; only the admin
        // service still wants the mutex-wrapped form.
        let policy_cache = Arc::new(tokio::sync::Mutex::new(redis.clone()));
        let access_control = Arc::new(build_access_control_service_with_cache_from_repos(
            repositories,
            redis.clone(),
        ));
        let policy_evaluation = Arc::new(build_policy_evaluation_service_with_cache_from_repos(
            repositories,
            redis,
        ));
        let admin_dependencies = AccessControlAdminDependencies {
            users: Arc::new(repositories.users.clone()),
            credentials: Arc::new(repositories.user_credentials.clone()),
            groups: Arc::new(repositories.groups.clone()),
            roles: Arc::new(repositories.roles.clone()),
            role_assignments: Arc::new(repositories.role_assignments.clone()),
            policies: Arc::new(repositories.policies.clone()),
            auth_config: settings.auth.clone(),
            secret_field_intents,
        };
        let access_control_admin = Arc::new(AccessControlAdminService::new_with_cache(
            admin_dependencies,
            policy_cache,
        ));
        let access_control_queries = Arc::new(AccessControlQueryService::new(
            Arc::new(repositories.roles.clone()),
            Arc::new(repositories.users.clone()),
            Arc::new(repositories.groups.clone()),
        ));

        PolicyServices {
            access_control,
            policy_evaluation,
            access_control_admin,
            access_control_queries,
        }
    }

    pub(super) fn build_inventory_and_sync_services(
        repositories: &AppRepositories,
        settings: &config::Settings,
        db_pool: &PgPool,
        audit: Arc<AuditService<SqlxAuditRepository>>,
        secret_store: Arc<DbSecretStore>,
    ) -> InventoryAndSyncServices {
        let sync_runtime = &settings.sync_runtime;
        let inventory_admin = Arc::new(InventoryAdminService::new(InventoryAdminRepositories {
            tenant_groups: Arc::new(repositories.tenant_groups.clone()),
            tenants: Arc::new(repositories.tenants.clone()),
            sites: Arc::new(repositories.sites.clone()),
            site_groups: Arc::new(repositories.site_groups.clone()),
            regions: Arc::new(repositories.regions.clone()),
            clusters: Arc::new(repositories.clusters.clone()),
            cluster_types: Arc::new(repositories.cluster_types.clone()),
            cluster_groups: Arc::new(repositories.cluster_groups.clone()),
            device_roles: Arc::new(repositories.device_roles.clone()),
            devices: Arc::new(repositories.devices.clone()),
            manufacturers: Arc::new(repositories.manufacturers.clone()),
            device_types: Arc::new(repositories.device_types.clone()),
            virtual_machines: Arc::new(repositories.virtual_machines.clone()),
        }));
        let backup = Arc::new(BackupService::new(db_pool.clone()));
        let ipam = Arc::new(IpamService::new(db_pool.clone()));
        let config_queries = Arc::new(ConfigQueryService::new(
            Arc::new(repositories.config_contexts.clone()),
            Arc::new(repositories.secrets.clone()),
        ));
        let sync_queries = Arc::new(SyncQueryService::new(
            Arc::new(repositories.sync_providers.clone()),
            Arc::new(repositories.sync_jobs.clone()),
        ));
        let source_service = Arc::new(SourceService::new(
            Arc::new(repositories.sync_providers.clone()),
            Arc::new(repositories.sync_source_runtime.clone()),
            Arc::new(repositories.sync_outbound_events.clone()),
            Arc::new(repositories.secrets.clone()),
            secret_store.clone(),
            SourceRuntimeConfig {
                callback_base_url: sync_runtime.callback_base_url.clone(),
                outbox_worker_batch_size: sync_runtime.outbox_worker_batch_size as usize,
            },
        ));
        let webhook_service = Arc::new(WebhookService::new(
            Arc::new(repositories.sync_source_runtime.clone()),
            Arc::new(repositories.sync_webhook_events.clone()),
            Arc::new(repositories.sites.clone()),
            audit,
        ));
        let outbound_service = Arc::new(OutboundService::new(
            Arc::new(repositories.sync_providers.clone()),
            Arc::new(repositories.sync_outbound_events.clone()),
            Arc::new(repositories.sync_source_runtime.clone()),
            Arc::new(repositories.secrets.clone()),
            secret_store,
            OutboundWorkerConfig {
                worker_batch_size: sync_runtime.outbox_worker_batch_size as usize,
                retry_max_attempts: sync_runtime.outbox_retry_max_attempts as i32,
                retry_backoff_ms: sync_runtime.outbox_retry_backoff_ms as i64,
            },
        ));

        InventoryAndSyncServices {
            inventory_admin,
            backup,
            ipam,
            config_queries,
            sync_queries,
            source_service,
            webhook_service,
            outbound_service,
        }
    }

    pub(super) fn build_license_services(
        settings: Arc<config::Settings>,
        db_pool: PgPool,
    ) -> LicenseServices {
        let licensing = Arc::new(LicensingService::new(settings, db_pool));
        let license_activation = Arc::new(LicenseActivationService::new(licensing.clone()));
        let license_status = Arc::new(LicenseStatusService::new(licensing.clone()));
        let license_login_gate = Arc::new(LicenseLoginGateService::new(licensing.clone()));

        LicenseServices {
            licensing,
            license_activation,
            license_status,
            license_login_gate,
        }
    }
}

impl AppServices {
    pub async fn new(
        settings: Arc<config::Settings>,
        repositories: &AppRepositories,
        db_pool: PgPool,
        redis: ConnectionManager,
    ) -> Result<Self, AppError> {
        let settings_ref = settings.as_ref();
        let auth_settings = &settings_ref.auth;
        let auth = Arc::new(AuthService::new(
            Arc::new(repositories.users.clone()),
            Arc::new(repositories.user_credentials.clone()),
            Arc::new(repositories.auth_sessions.clone()),
            auth_settings.clone(),
        ));
        // Outbound webhook delivery fans out from the single audit sink: every
        // recorded domain event is handed to the dispatcher on a best-effort
        // spawned task (failures never block or fail audit recording).
        let webhook_subscription_repository =
            crate::contexts::sync::infrastructure::persistence::repositories::webhook_subscription_repository::SqlxWebhookSubscriptionRepository::new(db_pool.clone());
        let webhook_dispatcher: Arc<dyn crate::contexts::audit::application::AuditEventFanout> =
            Arc::new(crate::contexts::sync::application::WebhookDispatcher::new(
                webhook_subscription_repository.clone(),
            ));
        let audit = Arc::new(AuditService::with_fanout(
            Arc::new(repositories.audit.clone()),
            webhook_dispatcher,
        ));
        let audit_query = Arc::new(AuditQueryService::new(Arc::new(repositories.audit.clone())));
        let user_api_tokens = Arc::new(UserApiTokenService::new(
            Arc::new(repositories.user_api_tokens.clone()),
            Arc::new(repositories.users.clone()),
            auth_settings.api_tokens.clone(),
            Some(audit.clone()),
        ));
        let settings_service = Arc::new(SettingsService::new(Arc::new(
            repositories.settings.clone(),
        )));
        let assistant = Arc::new(AssistantService::new(Arc::new(
            repositories.assistant.clone(),
        )));
        let materializer = Arc::new(MaterializationService::new(
            repositories.config_contexts.clone(),
            HostConfigCache::new(),
        ));
        let boot::SecretServices {
            secret_store,
            oidc_settings,
            secret_field_intents,
            secret_field_states,
        } = boot::build_secret_services(repositories, &db_pool);
        let boot::PolicyServices {
            access_control,
            policy_evaluation,
            access_control_admin,
            access_control_queries,
        } = boot::build_policy_services(
            repositories,
            settings_ref,
            redis,
            secret_field_intents.clone(),
        );
        let boot::InventoryAndSyncServices {
            inventory_admin,
            backup,
            ipam,
            config_queries,
            sync_queries,
            source_service,
            webhook_service,
            outbound_service,
        } = boot::build_inventory_and_sync_services(
            repositories,
            settings_ref,
            &db_pool,
            audit.clone(),
            secret_store.clone(),
        );
        let proxy_shared_secret_provider: Arc<dyn ProxySharedSecretProvider> =
            Arc::new(StaticProxySharedSecretProvider::ephemeral_embedded()?);
        let proxy_shared_secret = proxy_shared_secret_provider.get()?;
        let proxy_client = HttpProxyClient::new(
            config::effective_proxy_base_url(settings_ref),
            proxy_shared_secret,
        )?;
        let user_credential_sets = Arc::new(UserCredentialSetService::new(Arc::new(
            repositories.connections.clone(),
        )));
        let connection = Arc::new(ConnectionService::new(ConnectionServiceDeps {
            auth_session_ttl_seconds: std::num::NonZeroU64::new(
                settings_ref.auth.session_ttl_seconds,
            )
            .ok_or_else(|| {
                AppError::InvalidConfig("auth.session_ttl_seconds must be positive".to_string())
            })?,
            repository: Arc::new(repositories.connections.clone()),
            proxy_client: Arc::new(proxy_client),
            audit_service: audit.clone(),
            materializer: materializer.clone(),
            secret_repository: Arc::new(repositories.secrets.clone()),
            secret_store: secret_store.clone(),
            connconf_records: Arc::new(repositories.connconf_records.clone()),
            connconf_assignments: Arc::new(repositories.connconf_assignments.clone()),
        }));
        let wizard = Arc::new(WizardService::new(
            WizardProgressRepository::new(db_pool.clone()),
            db_pool.clone(),
        ));
        let boot::LicenseServices {
            licensing,
            license_activation,
            license_status,
            license_login_gate,
        } = boot::build_license_services(settings.clone(), db_pool.clone());

        let playbooks = Arc::new(
            PlaybookService::new_with_service_principal_authorizer(
                Arc::new(repositories.playbooks.clone()),
                access_control.clone(),
            )
            .with_scheduled_run_secret_authorizer(Arc::new(
                playbook_secret_gate::PlaybookScheduledRunSecretGate::new(
                    access_control.clone(),
                    Arc::new(repositories.secrets.clone()),
                ),
            )),
        );

        let inventory_templates = Arc::new(TemplateService::new(
            Arc::new(repositories.templates.clone()),
            Arc::new(repositories.template_step_resources.clone()),
        ));

        let inventory_layouts =
            Arc::new(LayoutService::new(Arc::new(repositories.layouts.clone())));

        let inventory_bulk_resources = Arc::new(BulkResourceService::new(Arc::new(
            repositories.bulk_resources.clone(),
        )));

        let custom_field_definitions_service = Arc::new(CustomFieldDefinitionService::new(
            Arc::new(repositories.custom_field_definitions.clone()),
            Arc::new(repositories.custom_field_choice_sets.clone()),
        ));
        let custom_field_values = Arc::new(CustomFieldValueService::new(
            Arc::new(repositories.custom_field_definitions.clone()),
            Arc::new(repositories.custom_field_choice_sets.clone()),
            Arc::new(repositories.secrets.clone()),
            secret_store.clone(),
            secret_field_intents.clone(),
        ));
        let custom_field_choice_sets = Arc::new(repositories.custom_field_choice_sets.clone());

        let sample_record_cache =
            Arc::new(crate::contexts::sync::application::SampleRecordCache::from_env());

        let webhook_subscriptions = Arc::new(
            crate::contexts::sync::application::WebhookSubscriptionService::new(
                webhook_subscription_repository,
            ),
        );

        Ok(Self {
            access_control,
            policy_evaluation,
            auth,
            user_api_tokens,
            settings: settings_service,
            assistant,
            playbooks,
            oidc_settings,
            materializer,
            secret_store,
            secret_field_intents,
            secret_field_states,
            custom_field_definitions: custom_field_definitions_service,
            custom_field_values,
            custom_field_choice_sets,
            proxy_shared_secret_provider,
            connection,
            user_credential_sets,
            wizard,
            audit,
            audit_query,
            access_control_admin,
            access_control_queries,
            inventory_admin,
            backup,
            ipam,
            config_queries,
            sync_queries,
            source_service,
            webhook_service,
            outbound_service,
            licensing,
            license_activation,
            license_status,
            license_login_gate,
            inventory_templates,
            inventory_layouts,
            inventory_bulk_resources,
            sample_record_cache,
            webhook_subscriptions,
        })
    }
}

/// Application state shared across all request handlers.
///
/// This state is cloned for each request handler via axum's State extractor.
/// All fields are designed to be cheap to clone (Arc, connection pools, etc.).
#[derive(Clone)]
pub struct AppState {
    /// Application configuration (settings, auth config, etc.)
    pub settings: Arc<config::Settings>,
    /// PostgreSQL connection pool for database operations
    pub db_pool: PgPool,
    /// Redis connection manager for caching and session storage
    pub redis: ConnectionManager,
    /// Shared repositories for data access
    pub repositories: AppRepositories,
    /// Shared application services
    pub services: AppServices,
    /// Proxy state when backend acts as proxy (proxy.url empty).
    /// Used for /proxy/v1/* and /ws/* routes.
    pub proxy_state: Option<Arc<proxy::ProxyState>>,
    /// Ephemeral browser-to-backend websocket leg health for live sessions.
    pub session_relay_health: SharedSessionRelayHealthRegistry,
    /// In-process fan-out of live playbook-run frames. The run callbacks
    /// publish persisted events/completions here; `ws_playbook_run`
    /// subscribers forward them to the browser.
    pub run_event_broadcaster: Arc<RunEventBroadcaster>,
}

impl AppState {
    /// Creates a new AppState with all required dependencies.
    ///
    /// The service container is built once at startup and reused for all requests.
    pub async fn build(
        settings: Arc<config::Settings>,
        db_pool: PgPool,
        redis: ConnectionManager,
    ) -> Result<Self, AppError> {
        let settings_ref = settings.as_ref();
        let secrets_settings = &settings_ref.secrets;
        crate::contexts::secrets::infrastructure::crypto::configure_cipher_key_material_ref(
            &secrets_settings.cipher.key_material_ref,
        )?;
        let repositories = AppRepositories::new(&db_pool);
        let services = AppServices::new(
            settings.clone(),
            &repositories,
            db_pool.clone(),
            redis.clone(),
        )
        .await?;
        crate::contexts::access_control::application::ensure_local_admin(
            &db_pool,
            &settings_ref.auth,
        )
        .await?;
        // Say out loud when /metrics will refuse every scrape. The endpoint
        // fails closed without a token, which is correct, but it takes the
        // monitoring down with it — and monitoring is exactly what would
        // otherwise have reported the outage.
        if let Some(warning) =
            metrics_scrape_disabled_warning(settings_ref.monitoring.metrics_token.as_deref())
        {
            log_event(
                LogLevel::Warning,
                "monitoring.metrics_endpoint.unscrapable",
                EventFields::default()
                    .with_operation("startup_config_check")
                    .with_status("degraded")
                    .with_context("warning", warning.to_string()),
            );
        }
        // Official playbook catalog: baked/overlay filesystem snapshot -> DB.
        // A broken refresh overlay degrades to the baked snapshot, and a baked
        // snapshot without a manifest degrades again to the local-development
        // catalog resolved against the working directory. Startup only fails
        // when none of the three loads — the product must not start without
        // its built-in system inventory playbook. Every degradation taken is
        // logged: the last one can succeed in a source checkout, which is how
        // an image built without its catalog would otherwise look healthy.
        let (catalog_dir, catalog, catalog_warnings) =
            official_catalog::load_configured_catalog(&settings_ref.official_catalog)?;
        for warning in catalog_warnings {
            log_event(
                LogLevel::Warning,
                "playbooks.official_catalog.degraded_load",
                EventFields::default()
                    .with_operation("official_catalog_load")
                    .with_status("degraded")
                    .with_context("warning", warning)
                    .with_context("catalog_dir", catalog_dir.display().to_string()),
            );
        }
        let synced_official = official_catalog::sync_official_catalog(&db_pool, &catalog).await?;
        for synced in &synced_official {
            log_event(
                LogLevel::Info,
                "playbooks.official_catalog.synced",
                EventFields::default()
                    .with_operation("official_catalog_sync")
                    .with_status("ok")
                    .with_context("slug", synced.slug.clone())
                    .with_context("playbook_id", synced.playbook_id.to_string())
                    .with_context("current_version_id", synced.version_id.to_string())
                    .with_context("system", synced.system.to_string()),
            );
        }
        let normalized_sessions = normalize_bootstrap_sessions(&db_pool).await?;
        log_event(
            LogLevel::Info,
            "connections.bootstrap.sessions_normalized",
            EventFields::default()
                .with_operation("bootstrap_sessions_normalize")
                .with_status("ok")
                .with_context("normalized_sessions", normalized_sessions.to_string()),
        );
        let proxy_state = Arc::new(proxy::ProxyState::new_with_provider(
            &settings_ref.proxy,
            services.proxy_shared_secret_provider.clone(),
        )?);
        // Reap SSH sessions whose client leg stays detached past the grace
        // window so abandoned PTYs / child `ssh` processes do not leak in the
        // in-process (backend-mode) proxy runtime.
        proxy::spawn_ssh_detach_reaper(proxy_state.clone());
        let proxy_state = Some(proxy_state);
        let session_relay_health = Arc::new(SessionRelayHealthRegistry::new());
        let run_event_broadcaster = Arc::new(RunEventBroadcaster::new());
        Ok(Self {
            settings: settings.clone(),
            db_pool,
            redis,
            repositories,
            services,
            proxy_state,
            session_relay_health,
            run_event_broadcaster,
        })
    }
}

async fn normalize_bootstrap_sessions(db_pool: &PgPool) -> Result<u64, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE connections_session
        SET status = 'disconnected'::connection_status,
            ended_at = COALESCE(ended_at, NOW()),
            last_activity = NOW()
        WHERE status IN ('pending'::connection_status, 'active'::connection_status)
        "#,
    )
    .execute(db_pool)
    .await
    .map_err(|error| {
        AppError::Database(format!(
            "Failed to normalize bootstrap connection sessions: {}",
            error
        ))
    })?;
    Ok(result.rows_affected())
}

/// Builds and returns the axum Router with all middleware and routes.
///
/// Middleware is applied in reverse order (bottom to top):
/// 1. Error middleware (catches and formats errors)
/// 2. Auth middleware (extracts user session from cookies)
/// 3. Request ID middleware (generates unique request IDs)
/// 4. Tracing middleware (adds request tracing spans)
/// 5. CORS middleware (handles cross-origin requests)
///
/// Routes:
/// - REST API routes from `build_rest_router()` (health, auth, etc.)
/// - Proxy routes (/proxy/v1/*)
pub fn build_router(state: AppState) -> Router {
    let settings = state.settings.as_ref();
    let max_body_bytes = settings.server.max_body_bytes;
    let rest_route_limits = RestRouteLimits {
        ssh_file_upload_body_bytes: RestRouteLimits::ssh_file_upload_body_bytes(
            settings.proxy.ssh_file_transfer.max_upload_bytes,
        ),
    };
    let cors_layer = build_cors_layer(settings);
    let mut app = Router::new().merge(build_rest_router_with_limits(rest_route_limits));

    if state.proxy_state.is_some() {
        app = app
            .route("/proxy/v1/connections", post(proxy_create_connection))
            .route(
                "/proxy/v1/connections/{connection_id}",
                delete(proxy_delete_connection),
            )
            .route(
                "/proxy/v1/connections/{connection_id}/metrics",
                get(proxy_get_metrics),
            )
            .route(
                "/proxy/v1/ws/terminal/{connection_id}",
                get(proxy_ws_terminal),
            )
            .route("/proxy/v1/ws/rdp/{connection_id}", get(proxy_ws_rdp))
            .route("/proxy/v1/ws/vnc/{connection_id}", get(proxy_ws_vnc));
    }

    app.layer(cors_layer)
        .layer(security_header_layer("x-frame-options", "DENY"))
        .layer(security_header_layer("x-content-type-options", "nosniff"))
        .layer(security_header_layer("referrer-policy", "no-referrer"))
        .layer(security_header_layer(
            "strict-transport-security",
            "max-age=63072000; includeSubDomains",
        ))
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(middleware::from_fn(tracing_middleware))
        .layer(middleware::from_fn(request_id_middleware))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn(error_middleware))
        .layer(middleware::from_fn(metrics_middleware))
        .with_state(state)
}

/// Builds a layer that sets a static security response header on every response.
///
/// Backend serves a JSON API; Content-Security-Policy is intentionally omitted
/// here and applied on the SPA (nginx) where it is meaningful.
fn security_header_layer(
    name: &'static str,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    )
}

fn require_proxy_state(
    state: &AppState,
) -> Result<Arc<proxy::ProxyState>, (StatusCode, &'static str)> {
    state
        .proxy_state
        .clone()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, PROXY_DISABLED_MESSAGE))
}

fn proxy_ws_route(
    state: &AppState,
    connection_id: Uuid,
    token: String,
    rdp_display_params: Option<proxy::RdpDisplayParams>,
    ws: axum::extract::ws::WebSocketUpgrade,
    protocols: &'static [&'static str],
) -> axum::response::Response {
    let proxy_state = match require_proxy_state(state) {
        Ok(proxy_state) => proxy_state,
        Err(response) => return response.into_response(),
    };
    proxy::ws_proxy_handler(
        proxy_state,
        connection_id,
        token,
        rdp_display_params,
        ws,
        protocols,
    )
}

async fn proxy_create_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    let proxy_state = match require_proxy_state(&state) {
        Ok(proxy_state) => proxy_state,
        Err(response) => return response.into_response(),
    };
    proxy::handle_create_connection(proxy_state, headers, payload).await
}

async fn proxy_delete_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(connection_id): Path<Uuid>,
) -> axum::response::Response {
    let proxy_state = match require_proxy_state(&state) {
        Ok(proxy_state) => proxy_state,
        Err(response) => return response.into_response(),
    };
    proxy::handle_delete_connection(proxy_state, headers, connection_id).await
}

async fn proxy_get_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(connection_id): Path<Uuid>,
) -> axum::response::Response {
    let proxy_state = match require_proxy_state(&state) {
        Ok(proxy_state) => proxy_state,
        Err(response) => return response.into_response(),
    };
    proxy::handle_get_metrics(proxy_state, headers, connection_id).await
}

async fn proxy_ws_terminal(
    State(state): State<AppState>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<proxy::ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    proxy_ws_route(
        &state,
        connection_id,
        query.token,
        None,
        ws,
        &PROXY_WS_PROTOCOL_GUACAMOLE,
    )
}

async fn proxy_ws_rdp(
    State(state): State<AppState>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<proxy::ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    proxy_ws_route(
        &state,
        connection_id,
        query.token,
        Some(proxy::RdpDisplayParams {
            width: query.width,
            height: query.height,
            dpi: query.dpi,
        }),
        ws,
        &PROXY_WS_PROTOCOL_GUACAMOLE,
    )
}

async fn proxy_ws_vnc(
    State(state): State<AppState>,
    Path(connection_id): Path<Uuid>,
    Query(query): Query<proxy::ProxyWsQuery>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    proxy_ws_route(
        &state,
        connection_id,
        query.token,
        None,
        ws,
        &PROXY_WS_PROTOCOL_VNC,
    )
}

/// Starts the axum HTTP server bound to the configured address.
///
/// The server will listen for incoming connections and handle:
/// - REST API requests (various endpoints)
/// - WebSocket connections (if configured)
///
/// The server runs until shutdown signal is received (SIGINT/SIGTERM).
pub async fn serve(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = build_router(state.clone());

    // Parse bind address from configuration (e.g., "0.0.0.0:8080")
    let addr = state
        .settings
        .server
        .bind
        .parse::<std::net::SocketAddr>()
        .map_err(|e| format!("Invalid bind address: {}", e))?;

    // Bind TCP listener to the address
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker_handles = spawn_background_workers(&state, shutdown_rx);

    // Start serving requests (blocks until server shutdown)
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        })
        .await?;

    for (event, operation, handle) in worker_handles {
        if let Err(error) = handle.await {
            log_error_event(
                event,
                "worker_join_failed",
                error.to_string(),
                EventFields::default()
                    .with_operation(operation)
                    .with_status("failed"),
            );
        }
    }

    state.db_pool.close().await;
    log_event(
        LogLevel::Info,
        "system.shutdown.completed",
        EventFields::default()
            .with_operation("server.shutdown")
            .with_status("ok"),
    );

    Ok(())
}

/// Keep worker join metadata with its task so every failed join is reported.
fn spawn_background_workers(
    state: &AppState,
    shutdown_rx: watch::Receiver<bool>,
) -> Vec<(&'static str, &'static str, tokio::task::JoinHandle<()>)> {
    vec![
        (
            "system.outbox_worker.join_failed",
            "outbox_worker",
            tokio::spawn(workers::run_outbox_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.webhook_prune_worker.join_failed",
            "webhook_prune_worker",
            tokio::spawn(workers::run_webhook_event_prune_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.journal_prune_worker.join_failed",
            "journal_prune_worker",
            tokio::spawn(workers::run_journal_prune_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.license_refresh_worker.join_failed",
            "license_refresh_worker",
            tokio::spawn(workers::run_license_refresh_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.playbook_run_worker.join_failed",
            "playbook_run_worker",
            tokio::spawn(workers::run_playbook_run_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.playbook_cancel_worker.join_failed",
            "playbook_cancel_worker",
            tokio::spawn(playbook_cancel_worker::run_playbook_cancel_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.observability_poller_worker.join_failed",
            "observability_poller_worker",
            tokio::spawn(workers::run_observability_poller_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.playbook_schedule_worker.join_failed",
            "playbook_schedule_worker",
            tokio::spawn(workers::run_playbook_schedule_worker(
                (*state).clone(),
                shutdown_rx.clone(),
            )),
        ),
        (
            "system.official_catalog_refresh_worker.join_failed",
            "official_catalog_refresh_worker",
            tokio::spawn(workers::run_official_catalog_refresh_worker(
                (*state).clone(),
                shutdown_rx,
            )),
        ),
    ]
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            log_error_event(
                "system.shutdown.ctrl_c_handler.install_failed",
                "shutdown_signal_handler_install_error",
                error.to_string(),
                EventFields::default()
                    .with_operation("server.shutdown")
                    .with_status("failed")
                    .with_context("signal", "ctrl_c"),
            );
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                log_error_event(
                    "system.shutdown.sigterm_handler.install_failed",
                    "shutdown_signal_handler_install_error",
                    error.to_string(),
                    EventFields::default()
                        .with_operation("server.shutdown")
                        .with_status("failed")
                        .with_context("signal", "sigterm"),
                );
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    log_event(
        LogLevel::Info,
        "system.shutdown.requested",
        EventFields::default()
            .with_operation("server.shutdown")
            .with_status("requested"),
    );
}
