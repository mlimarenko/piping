mod registry_collection_operations;

use std::collections::HashSet;

use piping_backend::api::rest::routes::monitoring;
use piping_backend::contexts::access_control::infrastructure::persistence::repositories::{
    repository as access_repos, user_api_token_repository,
};
use piping_backend::contexts::audit::infrastructure::persistence::repositories::audit_repository;
use piping_backend::contexts::connection_config::infrastructure::persistence::{
    assignment_repository as connconf_assignment_repo, record_repository as connconf_record_repo,
};
use piping_backend::contexts::connections::infrastructure::persistence::repositories::connection_repository;
use piping_backend::contexts::custom_fields::infrastructure::persistence::repositories::{
    custom_field_choice_set_repository, custom_field_definition_repository,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::{
    extras::config_context_repository, native::connection_target_repository,
    native::template_repository,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::repositories::playbook_repository;
use piping_backend::contexts::shared::list_query::{FilterKind, ListColumn, SortSpec};
use piping_backend::contexts::sync::infrastructure::persistence::repositories::{
    mapping_configuration_repository, provider_repository, webhook_subscription_repository,
};

use self::registry_collection_operations::{
    NonHttpFilter, RegistryRoute, assert_registry_collection_operations,
};

struct Registry {
    name: &'static str,
    const_name: &'static str,
    columns: &'static [ListColumn],
    path: &'static str,
}

const REGISTRIES: &[Registry] = &[
    Registry {
        name: "audit_events",
        const_name: "AUDIT_EVENT_COLUMNS",
        columns: audit_repository::AUDIT_EVENT_COLUMNS,
        path: "/api/v1/audit/events",
    },
    Registry {
        name: "access_users",
        const_name: "USER_COLUMNS",
        columns: access_repos::USER_COLUMNS,
        path: "/api/v1/admin/access-control/users",
    },
    Registry {
        name: "access_roles",
        const_name: "ROLE_COLUMNS",
        columns: access_repos::ROLE_COLUMNS,
        path: "/api/v1/admin/access-control/roles",
    },
    Registry {
        name: "access_groups",
        const_name: "GROUP_COLUMNS",
        columns: access_repos::GROUP_COLUMNS,
        path: "/api/v1/admin/access-control/groups",
    },
    Registry {
        name: "access_role_assignments",
        const_name: "ROLE_ASSIGNMENT_COLUMNS",
        columns: access_repos::ROLE_ASSIGNMENT_COLUMNS,
        path: "/api/v1/admin/access-control/role-assignments",
    },
    Registry {
        name: "access_policies",
        const_name: "POLICY_COLUMNS",
        columns: access_repos::POLICY_COLUMNS,
        path: "/api/v1/admin/access-control/policies",
    },
    Registry {
        name: "user_api_tokens",
        const_name: "USER_API_TOKEN_COLUMNS",
        columns: user_api_token_repository::USER_API_TOKEN_COLUMNS,
        path: "/api/v1/user/settings/api-tokens",
    },
    Registry {
        name: "connection_sessions",
        const_name: "CONNECTION_SESSION_COLUMNS",
        columns: connection_repository::CONNECTION_SESSION_COLUMNS,
        path: "/api/v1/connections/sessions",
    },
    Registry {
        name: "connection_templates",
        const_name: "CONNECTION_TEMPLATE_COLUMNS",
        columns: connection_repository::CONNECTION_TEMPLATE_COLUMNS,
        path: "/api/v1/connections/templates",
    },
    Registry {
        name: "proxy_nodes",
        const_name: "PROXY_NODE_COLUMNS",
        columns: connection_repository::PROXY_NODE_COLUMNS,
        path: "/api/v1/admin/proxy-nodes",
    },
    Registry {
        name: "proxy_node_assignments",
        const_name: "PROXY_NODE_ASSIGNMENT_COLUMNS",
        columns: connection_repository::PROXY_NODE_ASSIGNMENT_COLUMNS,
        path: "/api/v1/admin/proxy-node-assignments",
    },
    Registry {
        name: "user_credential_sets",
        const_name: "USER_CREDENTIAL_SET_COLUMNS",
        columns: connection_repository::USER_CREDENTIAL_SET_COLUMNS,
        path: "/api/v1/user/settings/credentials",
    },
    Registry {
        name: "connection_targets",
        const_name: "CONNECTION_TARGET_COLUMNS",
        columns: connection_target_repository::CONNECTION_TARGET_COLUMNS,
        path: "/api/v1/connections/targets",
    },
    Registry {
        name: "connection_config_records",
        const_name: "CONNCONF_RECORD_COLUMNS",
        columns: connconf_record_repo::CONNCONF_RECORD_COLUMNS,
        path: "/api/v1/connection-config/records",
    },
    Registry {
        name: "connection_config_assignments",
        const_name: "CONNCONF_ASSIGNMENT_COLUMNS",
        columns: connconf_assignment_repo::CONNCONF_ASSIGNMENT_COLUMNS,
        path: "/api/v1/connection-config/assignments",
    },
    Registry {
        name: "config_contexts",
        const_name: "CONFIG_CONTEXT_COLUMNS",
        columns: config_context_repository::CONFIG_CONTEXT_COLUMNS,
        path: "/api/v1/config-contexts",
    },
    Registry {
        name: "config_context_bindings",
        const_name: "CONFIG_CONTEXT_BINDING_COLUMNS",
        columns: config_context_repository::CONFIG_CONTEXT_BINDING_COLUMNS,
        path: "/api/v1/config-context-bindings",
    },
    Registry {
        name: "inventory_template_runs",
        const_name: "INVENTORY_TEMPLATE_RUN_COLUMNS",
        columns: template_repository::INVENTORY_TEMPLATE_RUN_COLUMNS,
        path: "/api/v1/inventory/template-runs",
    },
    Registry {
        name: "custom_field_definitions",
        const_name: "CUSTOM_FIELD_DEFINITION_COLUMNS",
        columns: custom_field_definition_repository::CUSTOM_FIELD_DEFINITION_COLUMNS,
        path: "/api/v1/custom-fields",
    },
    Registry {
        name: "custom_field_choice_sets",
        const_name: "CUSTOM_FIELD_CHOICE_SET_COLUMNS",
        columns: custom_field_choice_set_repository::CUSTOM_FIELD_CHOICE_SET_COLUMNS,
        path: "/api/v1/custom-field-choice-sets",
    },
    Registry {
        name: "playbook_runs",
        const_name: "PLAYBOOK_RUN_COLUMNS",
        columns: playbook_repository::PLAYBOOK_RUN_COLUMNS,
        path: "/api/v1/playbooks/runs",
    },
    Registry {
        name: "playbook_run_groups",
        const_name: "PLAYBOOK_RUN_GROUP_COLUMNS",
        columns: playbook_repository::PLAYBOOK_RUN_GROUP_COLUMNS,
        path: "/api/v1/playbooks/run-groups",
    },
    Registry {
        name: "playbook_assignments",
        const_name: "PLAYBOOK_ASSIGNMENT_COLUMNS",
        columns: playbook_repository::PLAYBOOK_ASSIGNMENT_COLUMNS,
        path: "/api/v1/playbooks/assignments",
    },
    Registry {
        name: "playbook_target_sets",
        const_name: "PLAYBOOK_TARGET_SET_COLUMNS",
        columns: playbook_repository::PLAYBOOK_TARGET_SET_COLUMNS,
        path: "/api/v1/playbooks/target-sets",
    },
    Registry {
        name: "playbook_schedules",
        const_name: "PLAYBOOK_SCHEDULE_COLUMNS",
        columns: playbook_repository::PLAYBOOK_SCHEDULE_COLUMNS,
        path: "/api/v1/playbooks/schedules",
    },
    Registry {
        name: "playbook_service_principals",
        const_name: "PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS",
        columns: playbook_repository::PLAYBOOK_SERVICE_PRINCIPAL_COLUMNS,
        path: "/api/v1/playbooks/service-principals",
    },
    Registry {
        name: "mapping_configurations",
        const_name: "MAPPING_CONFIGURATION_COLUMNS",
        columns: mapping_configuration_repository::MAPPING_CONFIGURATION_COLUMNS,
        path: "/api/v1/sync/mapping-configurations",
    },
    Registry {
        name: "provider_sources",
        const_name: "PROVIDER_COLUMNS",
        columns: provider_repository::PROVIDER_COLUMNS,
        path: "/api/v1/providers",
    },
    Registry {
        name: "observability_exporter_assignments",
        const_name: "repository::EXPORTER_ASSIGNMENT_COLUMNS",
        columns: monitoring::MONITORING_EXPORTER_ASSIGNMENT_COLUMNS,
        path: "/api/v1/monitoring/exporter-assignments",
    },
    Registry {
        name: "webhook_subscriptions",
        const_name: "WEBHOOK_SUBSCRIPTION_COLUMNS",
        columns: webhook_subscription_repository::WEBHOOK_SUBSCRIPTION_COLUMNS,
        path: "/api/v1/webhooks",
    },
];

/// Non-inventory registries whose declared filter is not a settable query
/// parameter, each with the reason that makes it so.
const NON_HTTP_FILTERS: &[NonHttpFilter] = &[NonHttpFilter {
    registry: "user_credential_sets",
    param: "user_id",
    reason: "current-user scope, not a query filter",
}];

fn assert_non_inventory_column_identity(
    registry: &Registry,
    column: &ListColumn,
    keys: &mut HashSet<&'static str>,
) {
    assert!(
        !column.key.trim().is_empty(),
        "{} has an empty column key",
        registry.name
    );
    assert!(
        !column.key.chars().any(char::is_whitespace),
        "{} has a whitespace-bearing column key: {:?}",
        registry.name,
        column.key
    );
    assert!(
        keys.insert(column.key),
        "{} has duplicate column key {:?}",
        registry.name,
        column.key
    );
    assert!(
        !column.label_key.trim().is_empty(),
        "{}.{} has an empty label_key",
        registry.name,
        column.key
    );
}

fn assert_non_inventory_sortable_column(
    registry: &Registry,
    column: &ListColumn,
    sortable_count: &mut usize,
) {
    if let Some(sort_sql) = column.sort_sql {
        *sortable_count += 1;
        assert!(
            !sort_sql.trim().is_empty(),
            "{}.{} has empty sort SQL",
            registry.name,
            column.key
        );
        assert!(
            SortSpec::parse_columns(Some(column.key), Some("asc"), registry.columns).is_ok(),
            "{} rejected its own asc sort key {:?}",
            registry.name,
            column.key
        );
        assert!(
            SortSpec::parse_columns(Some(column.key), Some("desc"), registry.columns).is_ok(),
            "{} rejected its own desc sort key {:?}",
            registry.name,
            column.key
        );
    }
}

fn assert_non_inventory_filter_column(registry: &Registry, column: &ListColumn) {
    if let Some(filter) = column.filter {
        assert!(
            !filter.param.trim().is_empty(),
            "{}.{} has an empty filter param",
            registry.name,
            column.key
        );
        assert!(
            !filter.sql.trim().is_empty(),
            "{}.{} has empty filter SQL",
            registry.name,
            column.key
        );
        assert_non_inventory_uuid_filter(registry, column, filter.param, filter.kind);
    }
}

fn assert_non_inventory_uuid_filter(
    registry: &Registry,
    column: &ListColumn,
    filter_param: &'static str,
    filter_kind: FilterKind,
) {
    if filter_kind == FilterKind::Uuid {
        assert!(
            column.fk_entity.is_some(),
            "{}.{} UUID filter {:?} must declare fk_entity metadata",
            registry.name,
            column.key,
            filter_param
        );
    }
}

fn validate_non_inventory_registry(registry: &Registry) {
    let mut keys = HashSet::new();
    let mut sortable_count = 0;

    for column in registry.columns {
        assert_non_inventory_column_identity(registry, column, &mut keys);
        assert_non_inventory_sortable_column(registry, column, &mut sortable_count);
        assert_non_inventory_filter_column(registry, column);
    }

    assert!(
        sortable_count > 0,
        "{} has no sortable columns",
        registry.name
    );
}

#[test]
fn non_inventory_table_column_registries_are_valid() {
    for registry in REGISTRIES {
        assert!(
            !registry.columns.is_empty(),
            "{} has no declared columns",
            registry.name
        );

        validate_non_inventory_registry(registry);

        assert!(
            SortSpec::parse_columns(Some("__unsupported__"), None, registry.columns).is_err(),
            "{} accepted an unsupported sort key",
            registry.name
        );
    }
}

/// The frontend table checks (`apps/web/src/app/tableContracts.test.ts`,
/// `apps/web/src/features/inventory/inventoryFilterBackendParity.test.ts`) parse
/// this table by `name` and `const_name` into maps keyed by those identifiers, so
/// a duplicate would silently drop a registry there.
#[test]
fn non_inventory_registry_identifier_keys_are_unique() {
    let mut names = HashSet::new();
    let mut const_names = HashSet::new();

    for registry in REGISTRIES {
        assert!(
            names.insert(registry.name),
            "duplicate registry name {:?}",
            registry.name
        );
        assert!(
            const_names.insert(registry.const_name),
            "duplicate registry const name {:?}",
            registry.const_name
        );
    }
}

/// Every non-inventory registry must be served by a documented `GET` collection
/// operation that declares the query parameters its columns filter on.
#[test]
fn non_inventory_registries_resolve_to_documented_collection_operations()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let routes: Vec<RegistryRoute> = REGISTRIES
        .iter()
        .map(|registry| RegistryRoute {
            name: registry.name,
            path: registry.path,
            columns: registry.columns,
        })
        .collect();

    assert_registry_collection_operations(&routes, NON_HTTP_FILTERS)
}
