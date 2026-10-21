pub(super) const PLAYBOOK_COLUMNS: &str = r#"
    id, slug, name, description, status, runtime, risk, owner_user_id,
    category, current_version_id, default_target_set_id, source_kind,
    vars_schema, connection_default, inventory_rest_enabled,
    created_by_user_id, updated_by_user_id, created_at, updated_at, metadata
"#;

pub(super) const VERSION_COLUMNS: &str = r#"
    id, playbook_id, version, language, runtime, entrypoint, content,
    content_sha256, input_schema, output_schema, token_policy, timeout_seconds,
    changelog, created_by_user_id, created_at, metadata
"#;

pub(super) const EXECUTION_ENVIRONMENT_COLUMNS: &str = r#"
    id, name, description, image, pull_policy, runner_group, enabled, env,
    settings, metadata, created_by_user_id, updated_by_user_id, created_at,
    updated_at
"#;

/// Read model for a playbook project. The SCM credential identity is projected
/// from the canonical `secret_usage_binding` row (PWA-53); there is no raw
/// credential column. Callers append `WHERE`/`ORDER BY` against the `p` alias.
pub(super) const PROJECT_SELECT: &str = r#"
    p.id, p.name, p.description, p.scm_type, p.scm_url, p.scm_branch, p.scm_ref,
    p.playbook_base_path, credential_binding.secret_id AS credential_id,
    p.default_execution_environment_id, p.sync_policy, p.sync_status,
    p.last_sync_revision, p.last_sync_error, p.last_synced_at, p.metadata,
    p.created_by_user_id, p.updated_by_user_id, p.created_at, p.updated_at
    FROM playbook_project p
    LEFT JOIN secret_usage_binding credential_binding
      ON credential_binding.consumer_type = 'playbook_project'
     AND credential_binding.consumer_id = p.id
     AND credential_binding.field_key = 'scm_credential'
"#;

pub(super) const JOB_TEMPLATE_COLUMNS: &str = r#"
    id, name, description, playbook_id, project_id, playbook_path,
    inventory_target_set_id, execution_environment_id, credential_refs,
    extra_vars, launch_options, ask_variables_on_launch, ask_limit_on_launch,
    ask_tags_on_launch, ask_skip_tags_on_launch, ask_credential_on_launch,
    ask_execution_environment_on_launch, enabled, metadata, created_by_user_id,
    updated_by_user_id, created_at, updated_at
"#;

pub(super) const TARGET_SET_COLUMNS: &str = r#"
    id, name, mode, entity_kind, selector, created_by_user_id,
    updated_by_user_id, created_at, updated_at
"#;

pub(super) const ASSIGNMENT_COLUMNS: &str = r#"
    id, playbook_id, target_set_id, default_version_id, name, scope_kind,
    scope_id, priority, enabled, visible, executable, quick_action_enabled,
    default_input, default_metadata, default_timeout_seconds, default_target_limit,
    created_by_user_id, updated_by_user_id, created_at, updated_at, metadata
"#;

pub(super) const ASSIGNMENT_COLUMNS_A: &str = r#"
    a.id AS id, a.playbook_id AS playbook_id, a.target_set_id AS target_set_id,
    a.default_version_id AS default_version_id, a.name AS name, a.scope_kind AS scope_kind,
    a.scope_id AS scope_id, a.priority AS priority, a.enabled AS enabled, a.visible AS visible,
    a.executable AS executable,
    (a.quick_action_enabled AND p.risk = 'low') AS quick_action_enabled,
    a.default_input AS default_input, a.default_metadata AS default_metadata,
    a.default_timeout_seconds AS default_timeout_seconds,
    a.default_target_limit AS default_target_limit, a.created_by_user_id AS created_by_user_id,
    a.updated_by_user_id AS updated_by_user_id, a.created_at AS created_at,
    a.updated_at AS updated_at, a.metadata AS metadata
"#;

pub(super) const SCHEDULE_COLUMNS: &str = r#"
    id, playbook_id, target_set_id, service_principal_id, name,
    cron_expression, timezone, misfire_policy, concurrency_policy, enabled,
    next_run_at, last_run_at, created_by_user_id, updated_by_user_id,
    created_at, updated_at, metadata
"#;

pub(super) const SERVICE_PRINCIPAL_COLUMNS: &str = r#"
    id, name, slug, description, status, created_by_user_id,
    updated_by_user_id, created_at, updated_at, revoked_at, revoked_by_user_id,
    revocation_reason, metadata
"#;

pub(super) const RUN_GROUP_COLUMNS: &str = r#"
    id, playbook_id, playbook_version_id, target_set_id, schedule_id, scheduled_for,
    requested_by_user_id, service_principal_id, trigger_kind, status,
    total_runs, succeeded_runs, failed_runs, canceled_runs, timeout_runs,
    created_at, updated_at, started_at, finished_at, lease_owner,
    lease_expires_at, leased_at, dispatch_id, dispatch_attempt,
    cancel_requested_at, cancel_deadline_at, cancel_outcome, metadata
"#;

pub(super) const RUN_COLUMNS: &str = r#"
    id, run_group_id, playbook_id, playbook_version_id, target_kind, target_id,
    source_kind, source_id, target_name, target_address, target_address_bound,
    credential_account_kind, credential_account_id, credential_account_entry_id,
    credential_merge_policy, credential_resolution, status, cancel_requested_at, cancel_outcome,
    lease_owner, lease_expires_at, queued_at, leased_at, started_at,
    finished_at, exit_code, error_message, attempt,
    timeout_seconds, metadata
"#;

pub(super) const JOB_TOKEN_COLUMNS: &str = r#"
    id, run_id, target_kind, target_id, source_kind, source_id, token_prefix,
    status, scopes, issued_at, expires_at, last_used_at, revoked_at, metadata
"#;

pub(super) const ARTIFACT_COLUMNS: &str = r#"
    id, run_id, artifact_type, name, content_type, content_json, content_text,
    content_sha256, content_size_bytes, created_at
"#;

pub(super) const RUN_EVENT_COLUMNS: &str = r#"
    id, run_group_id, run_id, host, event_sequence, event_kind, event_type, message, payload, created_at
"#;

pub(super) const TARGET_PREVIEW_BASE_SQL: &str = r#"
WITH dev AS (
    SELECT
        d.id                         AS target_id,
        d.name                       AS name,
        COALESCE(
            NULLIF(split_part(ip4.address::text, '/', 1), ''),
            NULLIF(split_part(ip6.address::text, '/', 1), ''),
            fallback_ip.address
        )                            AS address,
        'connection_target'::text    AS target_kind,
        'device'::text               AS source_kind,
        d.id                         AS source_id,
        CASE
            WHEN NULLIF(split_part(ip4.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.dcim_interface di
                    WHERE di.device_id = d.id
                      AND di.id = ip4.assigned_object_id
                ),
                FALSE
            )
            WHEN NULLIF(split_part(ip6.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.dcim_interface di
                    WHERE di.device_id = d.id
                      AND di.id = ip6.assigned_object_id
                ),
                FALSE
            )
            ELSE fallback_ip.address IS NOT NULL
        END                          AS address_bound,
        d.status::text               AS status,
        d.tags                       AS tags,
        d.tenant_id                  AS tenant_id,
        t.name                       AS tenant_name,
        d.site_id                    AS site_id,
        s.group_id                   AS site_group_id,
        d.location_id                AS location_id,
        d.rack_id                    AS rack_id,
        s.name                       AS site_name,
        s.region_id                  AS region_id,
        r.name                       AS region_name,
        d.role_id                    AS role_id,
        dr.name                      AS role_name,
        d.platform_id                AS platform_id,
        p.name                       AS platform_name,
        d.device_type_id             AS device_type_id,
        dt.model                     AS device_type_name,
        d.cluster_id                 AS cluster_id,
        c.name                       AS cluster_name
    FROM public.dcim_device d
    LEFT JOIN public.tenancy_tenant       t  ON t.id  = d.tenant_id
    LEFT JOIN public.dcim_site            s  ON s.id  = d.site_id
    LEFT JOIN public.dcim_region          r  ON r.id  = s.region_id
    LEFT JOIN public.dcim_devicerole      dr ON dr.id = d.role_id
    LEFT JOIN public.dcim_platform        p  ON p.id  = d.platform_id
    LEFT JOIN public.dcim_devicetype      dt ON dt.id = d.device_type_id
    LEFT JOIN public.virtualization_cluster c ON c.id = d.cluster_id
    LEFT JOIN public.ipam_ipaddress       ip4 ON ip4.id = d.primary_ip4_id
    LEFT JOIN public.ipam_ipaddress       ip6 ON ip6.id = d.primary_ip6_id
    LEFT JOIN LATERAL (
        SELECT host(ip.address) AS address
        FROM public.dcim_interface di
        JOIN public.ipam_ipaddress ip
          ON ip.assigned_object_id = di.id
        JOIN public.inventory_content_type ct
          ON ct.id = ip.assigned_object_type_id
         AND ct.app_label = 'dcim'
         AND ct.model = 'interface'
        WHERE di.device_id = d.id
        ORDER BY
            CASE WHEN family(ip.address) = 4 THEN 0 ELSE 1 END,
            COALESCE(di.name, ''),
            di.id,
            host(ip.address),
            masklen(ip.address),
            ip.id
        LIMIT 1
    ) fallback_ip ON TRUE
), vm AS (
    SELECT
        v.id                         AS target_id,
        v.name                       AS name,
        COALESCE(
            NULLIF(split_part(ip4.address::text, '/', 1), ''),
            NULLIF(split_part(ip6.address::text, '/', 1), ''),
            fallback_ip.address
        )                            AS address,
        'connection_target'::text    AS target_kind,
        'virtual_machine'::text      AS source_kind,
        v.id                         AS source_id,
        CASE
            WHEN NULLIF(split_part(ip4.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.virtualization_vminterface vi
                    WHERE vi.virtual_machine_id = v.id
                      AND vi.id = ip4.assigned_object_id
                ),
                FALSE
            )
            WHEN NULLIF(split_part(ip6.address::text, '/', 1), '') IS NOT NULL THEN COALESCE(
                EXISTS (
                    SELECT 1
                    FROM public.virtualization_vminterface vi
                    WHERE vi.virtual_machine_id = v.id
                      AND vi.id = ip6.assigned_object_id
                ),
                FALSE
            )
            ELSE fallback_ip.address IS NOT NULL
        END                          AS address_bound,
        v.status::text               AS status,
        v.tags                       AS tags,
        v.tenant_id                  AS tenant_id,
        t.name                       AS tenant_name,
        v.site_id                    AS site_id,
        s.group_id                   AS site_group_id,
        NULL::uuid                   AS location_id,
        NULL::uuid                   AS rack_id,
        s.name                       AS site_name,
        s.region_id                  AS region_id,
        r.name                       AS region_name,
        v.role_id                    AS role_id,
        dr.name                      AS role_name,
        v.platform_id                AS platform_id,
        p.name                       AS platform_name,
        NULL::uuid                   AS device_type_id,
        NULL::text                   AS device_type_name,
        v.cluster_id                 AS cluster_id,
        c.name                       AS cluster_name
    FROM public.virtualization_virtualmachine v
    LEFT JOIN public.tenancy_tenant       t  ON t.id  = v.tenant_id
    LEFT JOIN public.dcim_site            s  ON s.id  = v.site_id
    LEFT JOIN public.dcim_region          r  ON r.id  = s.region_id
    LEFT JOIN public.dcim_devicerole      dr ON dr.id = v.role_id
    LEFT JOIN public.dcim_platform        p  ON p.id  = v.platform_id
    LEFT JOIN public.virtualization_cluster c ON c.id = v.cluster_id
    LEFT JOIN public.ipam_ipaddress       ip4 ON ip4.id = v.primary_ip4_id
    LEFT JOIN public.ipam_ipaddress       ip6 ON ip6.id = v.primary_ip6_id
    LEFT JOIN LATERAL (
        SELECT host(ip.address) AS address
        FROM public.virtualization_vminterface vi
        JOIN public.ipam_ipaddress ip
          ON ip.assigned_object_id = vi.id
        JOIN public.inventory_content_type ct
          ON ct.id = ip.assigned_object_type_id
         AND ct.app_label = 'virtualization'
         AND ct.model = 'vminterface'
        WHERE vi.virtual_machine_id = v.id
        ORDER BY
            CASE WHEN family(ip.address) = 4 THEN 0 ELSE 1 END,
            COALESCE(vi.name, ''),
            vi.id,
            host(ip.address),
            masklen(ip.address),
            ip.id
        LIMIT 1
    ) fallback_ip ON TRUE
)
SELECT * FROM (SELECT * FROM dev UNION ALL SELECT * FROM vm) AS connection_targets
"#;
