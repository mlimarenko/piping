LOCK TABLE config_context, config_context_binding IN ACCESS EXCLUSIVE MODE;
LOCK TABLE inventory_tag, dcim_region, dcim_sitegroup, dcim_site, dcim_location,
    dcim_devicetype, dcim_devicerole, dcim_platform, dcim_device,
    virtualization_clustertype, virtualization_clustergroup, virtualization_cluster,
    virtualization_virtualmachine, tenancy_tenantgroup, tenancy_tenant IN SHARE MODE;

CREATE FUNCTION config_context_scope_exists(kind text, target uuid) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT CASE kind
        WHEN 'tag' THEN EXISTS (SELECT 1 FROM inventory_tag WHERE id = target)
        WHEN 'region' THEN EXISTS (SELECT 1 FROM dcim_region WHERE id = target)
        WHEN 'site_group' THEN EXISTS (SELECT 1 FROM dcim_sitegroup WHERE id = target)
        WHEN 'site' THEN EXISTS (SELECT 1 FROM dcim_site WHERE id = target)
        WHEN 'location' THEN EXISTS (SELECT 1 FROM dcim_location WHERE id = target)
        WHEN 'device_type' THEN EXISTS (SELECT 1 FROM dcim_devicetype WHERE id = target)
        WHEN 'role' THEN EXISTS (SELECT 1 FROM dcim_devicerole WHERE id = target)
        WHEN 'platform' THEN EXISTS (SELECT 1 FROM dcim_platform WHERE id = target)
        WHEN 'cluster_type' THEN EXISTS (SELECT 1 FROM virtualization_clustertype WHERE id = target)
        WHEN 'cluster_group' THEN EXISTS (SELECT 1 FROM virtualization_clustergroup WHERE id = target)
        WHEN 'cluster' THEN EXISTS (SELECT 1 FROM virtualization_cluster WHERE id = target)
        WHEN 'tenant_group' THEN EXISTS (SELECT 1 FROM tenancy_tenantgroup WHERE id = target)
        WHEN 'tenant' THEN EXISTS (SELECT 1 FROM tenancy_tenant WHERE id = target)
        WHEN 'device' THEN EXISTS (SELECT 1 FROM dcim_device WHERE id = target)
        WHEN 'vm' THEN EXISTS (SELECT 1 FROM virtualization_virtualmachine WHERE id = target)
        WHEN 'connection_target' THEN EXISTS (SELECT 1 FROM dcim_device WHERE id = target)
            OR EXISTS (SELECT 1 FROM virtualization_virtualmachine WHERE id = target)
        ELSE false END
$$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM config_context_binding WHERE scope_type NOT IN
        ('tenant_group', 'tenant', 'site', 'cluster', 'role', 'device', 'vm', 'connection_target')) THEN
        RAISE EXCEPTION 'config context has a previously inert binding that requires review';
    END IF;
    IF EXISTS (SELECT 1 FROM config_context WHERE jsonb_typeof(data) <> 'object') THEN
        RAISE EXCEPTION 'config context data must be a JSON object';
    END IF;
    IF EXISTS (SELECT 1 FROM config_context WHERE weight NOT BETWEEN 0 AND 32767) THEN
        RAISE EXCEPTION 'config context weight must be between 0 and 32767';
    END IF;
    IF EXISTS (SELECT 1 FROM config_context_binding WHERE NOT config_context_scope_exists(scope_type, scope_id)) THEN
        RAISE EXCEPTION 'config context has an unsupported or missing binding target';
    END IF;
    IF EXISTS (
        SELECT 1 FROM config_context c LEFT JOIN config_context_binding b ON b.context_id = c.id
        WHERE c.is_active GROUP BY c.id HAVING count(b.id) = 0 OR count(DISTINCT b.scope_type) > 1
    ) THEN
        RAISE EXCEPTION 'active config contexts with no bindings or multiple categories must be reviewed and deactivated before upgrade';
    END IF;
    IF EXISTS (
        SELECT 1 FROM config_context c
        JOIN config_context_binding b ON b.context_id = c.id AND b.scope_type = 'cluster'
        JOIN dcim_device d ON d.cluster_id = b.scope_id
        WHERE c.is_active
    ) THEN
        RAISE EXCEPTION 'active cluster config contexts with physical device members must be reviewed and deactivated before upgrade';
    END IF;
END $$;

ALTER TABLE config_context ADD CONSTRAINT config_context_data_object CHECK (jsonb_typeof(data) = 'object');
ALTER TABLE config_context ALTER COLUMN weight SET DEFAULT 1000;
ALTER TABLE config_context ADD CONSTRAINT config_context_weight_range CHECK (weight BETWEEN 0 AND 32767);
