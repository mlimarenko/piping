-- PIP-211 catalog extension. One SQLx transaction; append-only.
-- Preserves every existing dataflow id, name, status, traffic tuple and
-- selected-IP association. The old selector columns/table are replaced by
-- normalized selectors and re-keyed selected IPs. Any conservation failure
-- aborts before the incompatible drops.

SELECT dcim_acquire_hardware_graph();
SELECT pg_advisory_xact_lock(hashtextextended('inventory-dataflow-catalog', 0));
LOCK TABLE inventory_dataflow, inventory_dataflow_selected_ip IN ACCESS EXCLUSIVE MODE;

-- The old deferred checks and their function cannot survive the representation
-- change. They are dropped before any copy/drop so the transaction owns the
-- full invariant replacement.
DROP TRIGGER inventory_dataflow_selection_consistency ON inventory_dataflow;
DROP TRIGGER inventory_dataflow_selected_ip_consistency ON inventory_dataflow_selected_ip;
DROP FUNCTION inventory_dataflow_selection_check();

CREATE TABLE inventory_application_role (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text NOT NULL DEFAULT '',
    comments text NOT NULL DEFAULT '',
    owner_id uuid REFERENCES users_owner(id) ON DELETE RESTRICT,
    CONSTRAINT inventory_application_role_pkey PRIMARY KEY (id),
    CONSTRAINT inventory_application_role_name_check CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    CONSTRAINT inventory_application_role_description_check CHECK (length(description) <= 4000),
    CONSTRAINT inventory_application_role_comments_check CHECK (length(comments) <= 4000)
);
CREATE UNIQUE INDEX inventory_application_role_name_unique ON inventory_application_role (btrim(name));
CREATE INDEX inventory_application_role_owner_id ON inventory_application_role (owner_id);

CREATE TABLE inventory_application (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    role_id uuid REFERENCES inventory_application_role(id) ON DELETE RESTRICT,
    description text NOT NULL DEFAULT '',
    comments text NOT NULL DEFAULT '',
    owner_id uuid REFERENCES users_owner(id) ON DELETE RESTRICT,
    tenant_id uuid REFERENCES tenancy_tenant(id) ON DELETE RESTRICT,
    tags text[] NOT NULL DEFAULT '{}',
    custom_field_data jsonb NOT NULL DEFAULT '{}',
    CONSTRAINT inventory_application_pkey PRIMARY KEY (id),
    CONSTRAINT inventory_application_name_check CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    CONSTRAINT inventory_application_description_check CHECK (length(description) <= 4000),
    CONSTRAINT inventory_application_comments_check CHECK (length(comments) <= 4000)
);
CREATE UNIQUE INDEX inventory_application_name_unique ON inventory_application (btrim(name));
CREATE INDEX inventory_application_role_id ON inventory_application (role_id);
CREATE INDEX inventory_application_owner_id ON inventory_application (owner_id);
CREATE INDEX inventory_application_tenant_id ON inventory_application (tenant_id);

CREATE TABLE inventory_dataflow_group (
    id uuid DEFAULT uuidv7() NOT NULL,
    parent_id uuid REFERENCES inventory_dataflow_group(id) ON DELETE RESTRICT,
    application_id uuid REFERENCES inventory_application(id) ON DELETE RESTRICT,
    name text NOT NULL,
    description text NOT NULL DEFAULT '',
    comments text NOT NULL DEFAULT '',
    status text NOT NULL DEFAULT 'active',
    owner_id uuid REFERENCES users_owner(id) ON DELETE RESTRICT,
    tenant_id uuid REFERENCES tenancy_tenant(id) ON DELETE RESTRICT,
    tags text[] NOT NULL DEFAULT '{}',
    custom_field_data jsonb NOT NULL DEFAULT '{}',
    CONSTRAINT inventory_dataflow_group_pkey PRIMARY KEY (id),
    CONSTRAINT inventory_dataflow_group_name_check CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    CONSTRAINT inventory_dataflow_group_description_check CHECK (length(description) <= 4000),
    CONSTRAINT inventory_dataflow_group_comments_check CHECK (length(comments) <= 4000),
    CONSTRAINT inventory_dataflow_group_status_check CHECK (status IN ('draft', 'active', 'disabled'))
);
CREATE UNIQUE INDEX inventory_dataflow_group_child_name_unique
    ON inventory_dataflow_group (parent_id, btrim(name)) WHERE parent_id IS NOT NULL;
CREATE UNIQUE INDEX inventory_dataflow_group_root_name_unique
    ON inventory_dataflow_group (application_id, btrim(name)) NULLS NOT DISTINCT WHERE parent_id IS NULL;
CREATE INDEX inventory_dataflow_group_parent_id ON inventory_dataflow_group (parent_id);
CREATE INDEX inventory_dataflow_group_application_id ON inventory_dataflow_group (application_id);
CREATE INDEX inventory_dataflow_group_owner_id ON inventory_dataflow_group (owner_id);
CREATE INDEX inventory_dataflow_group_tenant_id ON inventory_dataflow_group (tenant_id);

CREATE TABLE inventory_address_group (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text NOT NULL DEFAULT '',
    comments text NOT NULL DEFAULT '',
    owner_id uuid REFERENCES users_owner(id) ON DELETE RESTRICT,
    tenant_id uuid REFERENCES tenancy_tenant(id) ON DELETE RESTRICT,
    tags text[] NOT NULL DEFAULT '{}',
    custom_field_data jsonb NOT NULL DEFAULT '{}',
    CONSTRAINT inventory_address_group_pkey PRIMARY KEY (id),
    CONSTRAINT inventory_address_group_name_check CHECK (length(btrim(name)) BETWEEN 1 AND 200),
    CONSTRAINT inventory_address_group_description_check CHECK (length(description) <= 4000),
    CONSTRAINT inventory_address_group_comments_check CHECK (length(comments) <= 4000)
);
CREATE UNIQUE INDEX inventory_address_group_name_unique ON inventory_address_group (btrim(name));
CREATE INDEX inventory_address_group_owner_id ON inventory_address_group (owner_id);
CREATE INDEX inventory_address_group_tenant_id ON inventory_address_group (tenant_id);

CREATE TABLE inventory_dataflow_selector (
    id uuid DEFAULT uuidv7() NOT NULL,
    dataflow_id uuid REFERENCES inventory_dataflow(id) ON DELETE CASCADE,
    group_id uuid REFERENCES inventory_address_group(id) ON DELETE CASCADE,
    side text NOT NULL,
    position smallint NOT NULL,
    object_type text NOT NULL,
    address_mode text NOT NULL DEFAULT 'default',
    tag_object_type text,
    device_id uuid REFERENCES dcim_device(id) ON DELETE RESTRICT,
    vm_id uuid REFERENCES virtualization_virtualmachine(id) ON DELETE RESTRICT,
    service_id uuid REFERENCES ipam_service(id) ON DELETE RESTRICT,
    ipaddress_id uuid REFERENCES ipam_ipaddress(id) ON DELETE RESTRICT,
    prefix_id uuid REFERENCES ipam_prefix(id) ON DELETE RESTRICT,
    iprange_id uuid REFERENCES ipam_iprange(id) ON DELETE RESTRICT,
    tag_id uuid REFERENCES inventory_tag(id) ON DELETE RESTRICT,
    addressgroup_id uuid REFERENCES inventory_address_group(id) ON DELETE RESTRICT,
    region_id uuid REFERENCES dcim_region(id) ON DELETE RESTRICT,
    site_id uuid REFERENCES dcim_site(id) ON DELETE RESTRICT,
    sitegroup_id uuid REFERENCES dcim_sitegroup(id) ON DELETE RESTRICT,
    devicerole_id uuid REFERENCES dcim_devicerole(id) ON DELETE RESTRICT,
    tenant_id uuid REFERENCES tenancy_tenant(id) ON DELETE RESTRICT,
    tenantgroup_id uuid REFERENCES tenancy_tenantgroup(id) ON DELETE RESTRICT,
    platform_id uuid REFERENCES dcim_platform(id) ON DELETE RESTRICT,
    manufacturer_id uuid REFERENCES dcim_manufacturer(id) ON DELETE RESTRICT,
    devicetype_id uuid REFERENCES dcim_devicetype(id) ON DELETE RESTRICT,
    rack_id uuid REFERENCES dcim_rack(id) ON DELETE RESTRICT,
    location_id uuid REFERENCES dcim_location(id) ON DELETE RESTRICT,
    cluster_id uuid REFERENCES virtualization_cluster(id) ON DELETE RESTRICT,
    clustergroup_id uuid REFERENCES virtualization_clustergroup(id) ON DELETE RESTRICT,
    clustertype_id uuid REFERENCES virtualization_clustertype(id) ON DELETE RESTRICT,
    CONSTRAINT inventory_dataflow_selector_pkey PRIMARY KEY (id),
    CONSTRAINT inventory_dataflow_selector_position_check CHECK (position >= 1),
    CONSTRAINT inventory_dataflow_selector_owner_check CHECK (
        (dataflow_id IS NOT NULL AND group_id IS NULL AND side IN ('source', 'destination'))
        OR (dataflow_id IS NULL AND group_id IS NOT NULL AND side = 'member')
    ),
    CONSTRAINT inventory_dataflow_selector_object_type_check CHECK (object_type IN (
        'all', 'internet', 'dcim.device', 'virtualization.virtualmachine', 'ipam.service',
        'ipam.ipaddress', 'ipam.prefix', 'ipam.iprange', 'extras.tag',
        'inventory.addressgroup', 'dcim.region', 'dcim.site', 'dcim.sitegroup',
        'dcim.devicerole', 'tenancy.tenant', 'tenancy.tenantgroup', 'dcim.platform',
        'dcim.manufacturer', 'dcim.devicetype', 'dcim.rack', 'dcim.location',
        'virtualization.cluster', 'virtualization.clustergroup', 'virtualization.clustertype'
    )),
    CONSTRAINT inventory_dataflow_selector_reference_check CHECK (
        CASE
            WHEN object_type IN ('all', 'internet') THEN num_nonnulls(
                device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id,
                addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id,
                tenantgroup_id, platform_id, manufacturer_id, devicetype_id, rack_id,
                location_id, cluster_id, clustergroup_id, clustertype_id) = 0
            ELSE num_nonnulls(
                device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id,
                addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id,
                tenantgroup_id, platform_id, manufacturer_id, devicetype_id, rack_id,
                location_id, cluster_id, clustergroup_id, clustertype_id) = 1
            AND CASE object_type
                WHEN 'dcim.device' THEN device_id IS NOT NULL
                WHEN 'virtualization.virtualmachine' THEN vm_id IS NOT NULL
                WHEN 'ipam.service' THEN service_id IS NOT NULL
                WHEN 'ipam.ipaddress' THEN ipaddress_id IS NOT NULL
                WHEN 'ipam.prefix' THEN prefix_id IS NOT NULL
                WHEN 'ipam.iprange' THEN iprange_id IS NOT NULL
                WHEN 'extras.tag' THEN tag_id IS NOT NULL
                WHEN 'inventory.addressgroup' THEN addressgroup_id IS NOT NULL
                WHEN 'dcim.region' THEN region_id IS NOT NULL
                WHEN 'dcim.site' THEN site_id IS NOT NULL
                WHEN 'dcim.sitegroup' THEN sitegroup_id IS NOT NULL
                WHEN 'dcim.devicerole' THEN devicerole_id IS NOT NULL
                WHEN 'tenancy.tenant' THEN tenant_id IS NOT NULL
                WHEN 'tenancy.tenantgroup' THEN tenantgroup_id IS NOT NULL
                WHEN 'dcim.platform' THEN platform_id IS NOT NULL
                WHEN 'dcim.manufacturer' THEN manufacturer_id IS NOT NULL
                WHEN 'dcim.devicetype' THEN devicetype_id IS NOT NULL
                WHEN 'dcim.rack' THEN rack_id IS NOT NULL
                WHEN 'dcim.location' THEN location_id IS NOT NULL
                WHEN 'virtualization.cluster' THEN cluster_id IS NOT NULL
                WHEN 'virtualization.clustergroup' THEN clustergroup_id IS NOT NULL
                WHEN 'virtualization.clustertype' THEN clustertype_id IS NOT NULL
                ELSE false
            END
        END
    ),
    CONSTRAINT inventory_dataflow_selector_address_mode_check CHECK (
        CASE object_type
            WHEN 'all' THEN address_mode = 'default'
            WHEN 'internet' THEN address_mode = 'default'
            WHEN 'ipam.ipaddress' THEN address_mode = 'default'
            WHEN 'ipam.prefix' THEN address_mode = 'default'
            WHEN 'ipam.iprange' THEN address_mode = 'default'
            WHEN 'inventory.addressgroup' THEN address_mode = 'default'
            WHEN 'extras.tag' THEN address_mode IN ('default', 'all', 'oob')
            ELSE address_mode IN ('default', 'selected', 'all', 'oob')
        END
    ),
    CONSTRAINT inventory_dataflow_selector_oob_object_type_check CHECK (
        address_mode <> 'oob' OR object_type IN ('dcim.device', 'extras.tag')
    ),
    CONSTRAINT inventory_dataflow_selector_tag_object_type_check CHECK (
        object_type = 'extras.tag'
        AND tag_object_type IN ('dcim.device', 'virtualization.virtualmachine')
        OR object_type <> 'extras.tag' AND tag_object_type IS NULL
    ),
    CONSTRAINT inventory_dataflow_selector_tag_oob_device_check CHECK (
        NOT (object_type = 'extras.tag' AND address_mode = 'oob' AND tag_object_type <> 'dcim.device')
    ),
    CONSTRAINT inventory_dataflow_selector_group_member_check CHECK (
        group_id IS NULL OR addressgroup_id IS NULL
    )
);
CREATE UNIQUE INDEX inventory_dataflow_selector_flow_position_unique
    ON inventory_dataflow_selector (dataflow_id, side, position) WHERE dataflow_id IS NOT NULL;
CREATE UNIQUE INDEX inventory_dataflow_selector_group_position_unique
    ON inventory_dataflow_selector (group_id, position) WHERE group_id IS NOT NULL;
CREATE INDEX inventory_dataflow_selector_dataflow_id ON inventory_dataflow_selector (dataflow_id);
CREATE INDEX inventory_dataflow_selector_group_id ON inventory_dataflow_selector (group_id);
CREATE INDEX inventory_dataflow_selector_device_id ON inventory_dataflow_selector (device_id);
CREATE INDEX inventory_dataflow_selector_vm_id ON inventory_dataflow_selector (vm_id);
CREATE INDEX inventory_dataflow_selector_service_id ON inventory_dataflow_selector (service_id);
CREATE INDEX inventory_dataflow_selector_ipaddress_id ON inventory_dataflow_selector (ipaddress_id);
CREATE INDEX inventory_dataflow_selector_prefix_id ON inventory_dataflow_selector (prefix_id);
CREATE INDEX inventory_dataflow_selector_iprange_id ON inventory_dataflow_selector (iprange_id);
CREATE INDEX inventory_dataflow_selector_tag_id ON inventory_dataflow_selector (tag_id);
CREATE INDEX inventory_dataflow_selector_addressgroup_id ON inventory_dataflow_selector (addressgroup_id);
CREATE INDEX inventory_dataflow_selector_region_id ON inventory_dataflow_selector (region_id);
CREATE INDEX inventory_dataflow_selector_site_id ON inventory_dataflow_selector (site_id);
CREATE INDEX inventory_dataflow_selector_sitegroup_id ON inventory_dataflow_selector (sitegroup_id);
CREATE INDEX inventory_dataflow_selector_devicerole_id ON inventory_dataflow_selector (devicerole_id);
CREATE INDEX inventory_dataflow_selector_tenant_id ON inventory_dataflow_selector (tenant_id);
CREATE INDEX inventory_dataflow_selector_tenantgroup_id ON inventory_dataflow_selector (tenantgroup_id);
CREATE INDEX inventory_dataflow_selector_platform_id ON inventory_dataflow_selector (platform_id);
CREATE INDEX inventory_dataflow_selector_manufacturer_id ON inventory_dataflow_selector (manufacturer_id);
CREATE INDEX inventory_dataflow_selector_devicetype_id ON inventory_dataflow_selector (devicetype_id);
CREATE INDEX inventory_dataflow_selector_rack_id ON inventory_dataflow_selector (rack_id);
CREATE INDEX inventory_dataflow_selector_location_id ON inventory_dataflow_selector (location_id);
CREATE INDEX inventory_dataflow_selector_cluster_id ON inventory_dataflow_selector (cluster_id);
CREATE INDEX inventory_dataflow_selector_clustergroup_id ON inventory_dataflow_selector (clustergroup_id);
CREATE INDEX inventory_dataflow_selector_clustertype_id ON inventory_dataflow_selector (clustertype_id);

CREATE TABLE inventory_dataflow_selector_ip (
    selector_id uuid NOT NULL REFERENCES inventory_dataflow_selector(id) ON DELETE CASCADE,
    ipaddress_id uuid NOT NULL REFERENCES ipam_ipaddress(id) ON DELETE RESTRICT,
    CONSTRAINT inventory_dataflow_selector_ip_pkey PRIMARY KEY (selector_id, ipaddress_id)
);
CREATE INDEX inventory_dataflow_selector_ip_ipaddress_id ON inventory_dataflow_selector_ip (ipaddress_id);

-- One selector row per old flow side, copying the matching typed reference only.
INSERT INTO inventory_dataflow_selector (
    dataflow_id, side, position, object_type, address_mode, tag_object_type,
    device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id,
    addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id,
    tenantgroup_id, platform_id, manufacturer_id, devicetype_id, rack_id,
    location_id, cluster_id, clustergroup_id, clustertype_id)
SELECT f.id, 'source', 1, f.source_kind, f.source_mode, NULL,
    CASE f.source_kind WHEN 'dcim.device' THEN f.source_device_id END,
    CASE f.source_kind WHEN 'virtualization.virtualmachine' THEN f.source_vm_id END,
    CASE f.source_kind WHEN 'ipam.service' THEN f.source_service_id END,
    CASE f.source_kind WHEN 'ipam.ipaddress' THEN f.source_ipaddress_id END,
    CASE f.source_kind WHEN 'ipam.prefix' THEN f.source_prefix_id END,
    NULL, NULL, NULL,
    CASE f.source_kind WHEN 'dcim.region' THEN f.source_region_id END,
    CASE f.source_kind WHEN 'dcim.site' THEN f.source_site_id END,
    CASE f.source_kind WHEN 'dcim.sitegroup' THEN f.source_sitegroup_id END,
    CASE f.source_kind WHEN 'dcim.devicerole' THEN f.source_devicerole_id END,
    CASE f.source_kind WHEN 'tenancy.tenant' THEN f.source_tenant_id END,
    CASE f.source_kind WHEN 'tenancy.tenantgroup' THEN f.source_tenantgroup_id END,
    CASE f.source_kind WHEN 'dcim.platform' THEN f.source_platform_id END,
    CASE f.source_kind WHEN 'dcim.manufacturer' THEN f.source_manufacturer_id END,
    CASE f.source_kind WHEN 'dcim.devicetype' THEN f.source_devicetype_id END,
    CASE f.source_kind WHEN 'dcim.rack' THEN f.source_rack_id END,
    CASE f.source_kind WHEN 'dcim.location' THEN f.source_location_id END,
    CASE f.source_kind WHEN 'virtualization.cluster' THEN f.source_cluster_id END,
    CASE f.source_kind WHEN 'virtualization.clustergroup' THEN f.source_clustergroup_id END,
    CASE f.source_kind WHEN 'virtualization.clustertype' THEN f.source_clustertype_id END
FROM inventory_dataflow f;
INSERT INTO inventory_dataflow_selector (
    dataflow_id, side, position, object_type, address_mode, tag_object_type,
    device_id, vm_id, service_id, ipaddress_id, prefix_id, iprange_id, tag_id,
    addressgroup_id, region_id, site_id, sitegroup_id, devicerole_id, tenant_id,
    tenantgroup_id, platform_id, manufacturer_id, devicetype_id, rack_id,
    location_id, cluster_id, clustergroup_id, clustertype_id)
SELECT f.id, 'destination', 1, f.destination_kind, f.destination_mode, NULL,
    CASE f.destination_kind WHEN 'dcim.device' THEN f.destination_device_id END,
    CASE f.destination_kind WHEN 'virtualization.virtualmachine' THEN f.destination_vm_id END,
    CASE f.destination_kind WHEN 'ipam.service' THEN f.destination_service_id END,
    CASE f.destination_kind WHEN 'ipam.ipaddress' THEN f.destination_ipaddress_id END,
    CASE f.destination_kind WHEN 'ipam.prefix' THEN f.destination_prefix_id END,
    NULL, NULL, NULL,
    CASE f.destination_kind WHEN 'dcim.region' THEN f.destination_region_id END,
    CASE f.destination_kind WHEN 'dcim.site' THEN f.destination_site_id END,
    CASE f.destination_kind WHEN 'dcim.sitegroup' THEN f.destination_sitegroup_id END,
    CASE f.destination_kind WHEN 'dcim.devicerole' THEN f.destination_devicerole_id END,
    CASE f.destination_kind WHEN 'tenancy.tenant' THEN f.destination_tenant_id END,
    CASE f.destination_kind WHEN 'tenancy.tenantgroup' THEN f.destination_tenantgroup_id END,
    CASE f.destination_kind WHEN 'dcim.platform' THEN f.destination_platform_id END,
    CASE f.destination_kind WHEN 'dcim.manufacturer' THEN f.destination_manufacturer_id END,
    CASE f.destination_kind WHEN 'dcim.devicetype' THEN f.destination_devicetype_id END,
    CASE f.destination_kind WHEN 'dcim.rack' THEN f.destination_rack_id END,
    CASE f.destination_kind WHEN 'dcim.location' THEN f.destination_location_id END,
    CASE f.destination_kind WHEN 'virtualization.cluster' THEN f.destination_cluster_id END,
    CASE f.destination_kind WHEN 'virtualization.clustergroup' THEN f.destination_clustergroup_id END,
    CASE f.destination_kind WHEN 'virtualization.clustertype' THEN f.destination_clustertype_id END
FROM inventory_dataflow f;

INSERT INTO inventory_dataflow_selector_ip (selector_id, ipaddress_id)
SELECT s.id, j.ipaddress_id
FROM inventory_dataflow_selected_ip j
JOIN inventory_dataflow_selector s ON s.dataflow_id = j.dataflow_id AND s.side = j.side
WHERE j.side IN ('source', 'destination');

-- Pre-drop conservation: no flow, selector or selected association may vanish
-- or be duplicated, and the copied kind/mode/typed reference must be exact.
CREATE TEMP TABLE pip211_dataflow_shape ON COMMIT DROP AS
SELECT id, name, description, status, transport, destination_ports, allow_cross_vrf
FROM inventory_dataflow;

DO $dataflow_conservation$
DECLARE
    flow_count bigint;
    selector_count bigint;
    old_selected_count bigint;
    new_selected_count bigint;
BEGIN
    SELECT count(*) INTO flow_count FROM inventory_dataflow;
    SELECT count(*) INTO selector_count
    FROM inventory_dataflow_selector
    WHERE dataflow_id IS NOT NULL AND side IN ('source', 'destination');
    IF selector_count <> 2 * flow_count THEN
        RAISE EXCEPTION 'Dataflow copy did not produce exactly two selectors per flow' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow f
        LEFT JOIN inventory_dataflow_selector s ON s.dataflow_id = f.id
        GROUP BY f.id
        HAVING count(s.id) <> 2
            OR count(*) FILTER (WHERE s.side = 'source') <> 1
            OR count(*) FILTER (WHERE s.side = 'destination') <> 1
            OR min(s.position) <> 1 OR max(s.position) <> 1
    ) THEN
        RAISE EXCEPTION 'Dataflow copy lost or duplicated an endpoint' USING ERRCODE = '23514';
    END IF;
    SELECT count(*) INTO old_selected_count FROM inventory_dataflow_selected_ip;
    SELECT count(*) INTO new_selected_count FROM inventory_dataflow_selector_ip;
    IF old_selected_count <> new_selected_count THEN
        RAISE EXCEPTION 'Dataflow selected-IP rows were lost or duplicated' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow_selected_ip j
        LEFT JOIN inventory_dataflow_selector s
            ON s.dataflow_id = j.dataflow_id AND s.side = j.side
        LEFT JOIN inventory_dataflow_selector_ip n
            ON n.selector_id = s.id AND n.ipaddress_id = j.ipaddress_id
        WHERE s.id IS NULL OR n.selector_id IS NULL
    ) THEN
        RAISE EXCEPTION 'A dataflow selected IP is missing its generated selector' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow_selector_ip n
        JOIN inventory_dataflow_selector s ON s.id = n.selector_id
        LEFT JOIN inventory_dataflow_selected_ip j
            ON j.dataflow_id = s.dataflow_id AND j.side = s.side AND j.ipaddress_id = n.ipaddress_id
        WHERE s.dataflow_id IS NOT NULL AND j.dataflow_id IS NULL
    ) THEN
        RAISE EXCEPTION 'A generated selected IP has no pre-DDL association' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow_selector s
        WHERE s.dataflow_id IS NOT NULL
          AND ((s.address_mode = 'selected')
               <> EXISTS (SELECT 1 FROM inventory_dataflow_selector_ip n WHERE n.selector_id = s.id))
    ) THEN
        RAISE EXCEPTION 'Selected mode and selected IPs are inconsistent after copy' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow f
        JOIN inventory_dataflow_selector s ON s.dataflow_id = f.id AND s.side = 'source'
        WHERE s.object_type IS DISTINCT FROM f.source_kind
           OR s.address_mode IS DISTINCT FROM f.source_mode
           OR s.tag_object_type IS NOT NULL
           OR (CASE s.object_type
                WHEN 'dcim.device' THEN s.device_id
                WHEN 'virtualization.virtualmachine' THEN s.vm_id
                WHEN 'ipam.service' THEN s.service_id
                WHEN 'ipam.ipaddress' THEN s.ipaddress_id
                WHEN 'ipam.prefix' THEN s.prefix_id
                WHEN 'dcim.region' THEN s.region_id
                WHEN 'dcim.site' THEN s.site_id
                WHEN 'dcim.sitegroup' THEN s.sitegroup_id
                WHEN 'dcim.devicerole' THEN s.devicerole_id
                WHEN 'tenancy.tenant' THEN s.tenant_id
                WHEN 'tenancy.tenantgroup' THEN s.tenantgroup_id
                WHEN 'dcim.platform' THEN s.platform_id
                WHEN 'dcim.manufacturer' THEN s.manufacturer_id
                WHEN 'dcim.devicetype' THEN s.devicetype_id
                WHEN 'dcim.rack' THEN s.rack_id
                WHEN 'dcim.location' THEN s.location_id
                WHEN 'virtualization.cluster' THEN s.cluster_id
                WHEN 'virtualization.clustergroup' THEN s.clustergroup_id
                WHEN 'virtualization.clustertype' THEN s.clustertype_id
               END)
              IS DISTINCT FROM
              (CASE f.source_kind
                WHEN 'dcim.device' THEN f.source_device_id
                WHEN 'virtualization.virtualmachine' THEN f.source_vm_id
                WHEN 'ipam.service' THEN f.source_service_id
                WHEN 'ipam.ipaddress' THEN f.source_ipaddress_id
                WHEN 'ipam.prefix' THEN f.source_prefix_id
                WHEN 'dcim.region' THEN f.source_region_id
                WHEN 'dcim.site' THEN f.source_site_id
                WHEN 'dcim.sitegroup' THEN f.source_sitegroup_id
                WHEN 'dcim.devicerole' THEN f.source_devicerole_id
                WHEN 'tenancy.tenant' THEN f.source_tenant_id
                WHEN 'tenancy.tenantgroup' THEN f.source_tenantgroup_id
                WHEN 'dcim.platform' THEN f.source_platform_id
                WHEN 'dcim.manufacturer' THEN f.source_manufacturer_id
                WHEN 'dcim.devicetype' THEN f.source_devicetype_id
                WHEN 'dcim.rack' THEN f.source_rack_id
                WHEN 'dcim.location' THEN f.source_location_id
                WHEN 'virtualization.cluster' THEN f.source_cluster_id
                WHEN 'virtualization.clustergroup' THEN f.source_clustergroup_id
                WHEN 'virtualization.clustertype' THEN f.source_clustertype_id
               END)
    ) THEN
        RAISE EXCEPTION 'Source selector kind, mode or typed reference changed during copy' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM inventory_dataflow f
        JOIN inventory_dataflow_selector s ON s.dataflow_id = f.id AND s.side = 'destination'
        WHERE s.object_type IS DISTINCT FROM f.destination_kind
           OR s.address_mode IS DISTINCT FROM f.destination_mode
           OR s.tag_object_type IS NOT NULL
           OR (CASE s.object_type
                WHEN 'dcim.device' THEN s.device_id
                WHEN 'virtualization.virtualmachine' THEN s.vm_id
                WHEN 'ipam.service' THEN s.service_id
                WHEN 'ipam.ipaddress' THEN s.ipaddress_id
                WHEN 'ipam.prefix' THEN s.prefix_id
                WHEN 'dcim.region' THEN s.region_id
                WHEN 'dcim.site' THEN s.site_id
                WHEN 'dcim.sitegroup' THEN s.sitegroup_id
                WHEN 'dcim.devicerole' THEN s.devicerole_id
                WHEN 'tenancy.tenant' THEN s.tenant_id
                WHEN 'tenancy.tenantgroup' THEN s.tenantgroup_id
                WHEN 'dcim.platform' THEN s.platform_id
                WHEN 'dcim.manufacturer' THEN s.manufacturer_id
                WHEN 'dcim.devicetype' THEN s.devicetype_id
                WHEN 'dcim.rack' THEN s.rack_id
                WHEN 'dcim.location' THEN s.location_id
                WHEN 'virtualization.cluster' THEN s.cluster_id
                WHEN 'virtualization.clustergroup' THEN s.clustergroup_id
                WHEN 'virtualization.clustertype' THEN s.clustertype_id
               END)
              IS DISTINCT FROM
              (CASE f.destination_kind
                WHEN 'dcim.device' THEN f.destination_device_id
                WHEN 'virtualization.virtualmachine' THEN f.destination_vm_id
                WHEN 'ipam.service' THEN f.destination_service_id
                WHEN 'ipam.ipaddress' THEN f.destination_ipaddress_id
                WHEN 'ipam.prefix' THEN f.destination_prefix_id
                WHEN 'dcim.region' THEN f.destination_region_id
                WHEN 'dcim.site' THEN f.destination_site_id
                WHEN 'dcim.sitegroup' THEN f.destination_sitegroup_id
                WHEN 'dcim.devicerole' THEN f.destination_devicerole_id
                WHEN 'tenancy.tenant' THEN f.destination_tenant_id
                WHEN 'tenancy.tenantgroup' THEN f.destination_tenantgroup_id
                WHEN 'dcim.platform' THEN f.destination_platform_id
                WHEN 'dcim.manufacturer' THEN f.destination_manufacturer_id
                WHEN 'dcim.devicetype' THEN f.destination_devicetype_id
                WHEN 'dcim.rack' THEN f.destination_rack_id
                WHEN 'dcim.location' THEN f.destination_location_id
                WHEN 'virtualization.cluster' THEN f.destination_cluster_id
                WHEN 'virtualization.clustergroup' THEN f.destination_clustergroup_id
                WHEN 'virtualization.clustertype' THEN f.destination_clustertype_id
               END)
    ) THEN
        RAISE EXCEPTION 'Destination selector kind, mode or typed reference changed during copy' USING ERRCODE = '23514';
    END IF;
END
$dataflow_conservation$;

ALTER TABLE inventory_dataflow
    ADD COLUMN application_id uuid REFERENCES inventory_application(id) ON DELETE RESTRICT,
    ADD COLUMN group_id uuid REFERENCES inventory_dataflow_group(id) ON DELETE RESTRICT,
    ADD COLUMN source_ports integer[],
    ADD COLUMN icmp_types integer[],
    ADD COLUMN comments text NOT NULL DEFAULT '',
    ADD COLUMN tenant_id uuid REFERENCES tenancy_tenant(id) ON DELETE RESTRICT,
    ADD COLUMN owner_id uuid REFERENCES users_owner(id) ON DELETE RESTRICT,
    ADD COLUMN tags text[] NOT NULL DEFAULT '{}',
    ADD COLUMN custom_field_data jsonb NOT NULL DEFAULT '{}';

ALTER TABLE inventory_dataflow DROP CONSTRAINT inventory_dataflow_transport_check;
ALTER TABLE inventory_dataflow ADD CONSTRAINT inventory_dataflow_transport_check
    CHECK (transport IN ('any', 'tcp', 'udp', 'tcp_udp', 'sctp', 'icmp', 'icmpv6'));

CREATE FUNCTION inventory_integer_array_unique(items integer[]) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT cardinality(items) = (SELECT count(DISTINCT item) FROM unnest(items) AS item)
$$;

ALTER TABLE inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_comments_check CHECK (length(comments) <= 4000),
    ADD CONSTRAINT inventory_dataflow_source_ports_check CHECK (
        cardinality(source_ports) BETWEEN 1 AND 256
        AND array_position(source_ports, NULL) IS NULL
        AND 1 <= ALL(source_ports) AND 65535 >= ALL(source_ports)),
    ADD CONSTRAINT inventory_dataflow_icmp_types_check CHECK (
        cardinality(icmp_types) BETWEEN 1 AND 256
        AND array_position(icmp_types, NULL) IS NULL
        AND 0 <= ALL(icmp_types) AND 255 >= ALL(icmp_types)
        AND inventory_integer_array_unique(icmp_types)),
    ADD CONSTRAINT inventory_dataflow_icmp_ports_check CHECK (
        transport NOT IN ('icmp', 'icmpv6')
        OR (destination_ports IS NULL AND source_ports IS NULL)),
    ADD CONSTRAINT inventory_dataflow_icmp_types_transport_check CHECK (
        transport IN ('icmp', 'icmpv6') OR icmp_types IS NULL);

CREATE INDEX inventory_dataflow_application_id ON inventory_dataflow (application_id);
CREATE INDEX inventory_dataflow_group_id ON inventory_dataflow (group_id);
CREATE INDEX inventory_dataflow_tenant_id ON inventory_dataflow (tenant_id);
CREATE INDEX inventory_dataflow_owner_id ON inventory_dataflow (owner_id);

-- The old junction has no dependents: its indexes, foreign keys and primary key
-- are owned by itself, so no CASCADE is required. Column drops likewise own
-- their indexes/constraints; no CASCADE is used for them either.
DROP TABLE inventory_dataflow_selected_ip;
ALTER TABLE inventory_dataflow
    DROP COLUMN source_kind,
    DROP COLUMN destination_kind,
    DROP COLUMN source_mode,
    DROP COLUMN destination_mode,
    DROP COLUMN source_device_id,
    DROP COLUMN source_vm_id,
    DROP COLUMN source_service_id,
    DROP COLUMN source_ipaddress_id,
    DROP COLUMN source_prefix_id,
    DROP COLUMN destination_device_id,
    DROP COLUMN destination_vm_id,
    DROP COLUMN destination_service_id,
    DROP COLUMN destination_ipaddress_id,
    DROP COLUMN destination_prefix_id,
    DROP COLUMN source_region_id,
    DROP COLUMN source_site_id,
    DROP COLUMN source_sitegroup_id,
    DROP COLUMN source_devicerole_id,
    DROP COLUMN source_tenant_id,
    DROP COLUMN source_tenantgroup_id,
    DROP COLUMN source_platform_id,
    DROP COLUMN source_manufacturer_id,
    DROP COLUMN source_devicetype_id,
    DROP COLUMN source_rack_id,
    DROP COLUMN source_location_id,
    DROP COLUMN source_cluster_id,
    DROP COLUMN source_clustergroup_id,
    DROP COLUMN source_clustertype_id,
    DROP COLUMN destination_region_id,
    DROP COLUMN destination_site_id,
    DROP COLUMN destination_sitegroup_id,
    DROP COLUMN destination_devicerole_id,
    DROP COLUMN destination_tenant_id,
    DROP COLUMN destination_tenantgroup_id,
    DROP COLUMN destination_platform_id,
    DROP COLUMN destination_manufacturer_id,
    DROP COLUMN destination_devicetype_id,
    DROP COLUMN destination_rack_id,
    DROP COLUMN destination_location_id,
    DROP COLUMN destination_cluster_id,
    DROP COLUMN destination_clustergroup_id,
    DROP COLUMN destination_clustertype_id;

-- Flow scalars were never rewritten; prove it after the incompatible drops.
DO $dataflow_shape_conservation$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pip211_dataflow_shape b
        FULL JOIN inventory_dataflow a USING (id)
        WHERE a.id IS NULL OR b.id IS NULL
           OR a.name IS DISTINCT FROM b.name
           OR a.description IS DISTINCT FROM b.description
           OR a.status IS DISTINCT FROM b.status
           OR a.transport IS DISTINCT FROM b.transport
           OR a.destination_ports IS DISTINCT FROM b.destination_ports
           OR a.allow_cross_vrf IS DISTINCT FROM b.allow_cross_vrf
    ) THEN
        RAISE EXCEPTION 'Dataflow identity, status or traffic changed during migration' USING ERRCODE = '23514';
    END IF;
END
$dataflow_shape_conservation$;

CREATE FUNCTION inventory_dataflow_selection_check() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE selector_ids uuid[]; wanted_selector uuid; state record;
BEGIN
    IF TG_TABLE_NAME = 'inventory_dataflow_selector' THEN
        IF TG_OP = 'INSERT' THEN
            selector_ids := ARRAY[NEW.id];
        ELSIF TG_OP = 'DELETE' THEN
            selector_ids := ARRAY[OLD.id];
        ELSE
            selector_ids := ARRAY[OLD.id, NEW.id];
        END IF;
    ELSE
        IF TG_OP = 'INSERT' THEN
            selector_ids := ARRAY[NEW.selector_id];
        ELSIF TG_OP = 'DELETE' THEN
            selector_ids := ARRAY[OLD.selector_id];
        ELSE
            selector_ids := ARRAY[OLD.selector_id, NEW.selector_id];
        END IF;
    END IF;
    FOREACH wanted_selector IN ARRAY selector_ids LOOP
        SELECT address_mode, dataflow_id, group_id INTO state
        FROM inventory_dataflow_selector WHERE id = wanted_selector;
        IF FOUND AND ((state.address_mode = 'selected')
                      <> EXISTS (SELECT 1 FROM inventory_dataflow_selector_ip WHERE selector_id = wanted_selector)) THEN
            RAISE EXCEPTION 'Selected endpoint mode must have selected IPs, other modes must not'
                USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END
$$;
CREATE CONSTRAINT TRIGGER inventory_dataflow_selector_selection_consistency
    AFTER INSERT OR UPDATE OR DELETE ON inventory_dataflow_selector
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION inventory_dataflow_selection_check();
CREATE CONSTRAINT TRIGGER inventory_dataflow_selector_ip_selection_consistency
    AFTER INSERT OR UPDATE OR DELETE ON inventory_dataflow_selector_ip
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW
    EXECUTE FUNCTION inventory_dataflow_selection_check();

CREATE FUNCTION inventory_dataflow_catalog_lock() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Dataflow catalog mutations require read committed isolation' USING ERRCODE = '23514';
    END IF;
    IF TG_TABLE_NAME = 'inventory_dataflow_group' THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('inventory-dataflow-catalog', 0));
    ELSE
        PERFORM pg_advisory_xact_lock_shared(hashtextextended('inventory-dataflow-catalog', 0));
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION inventory_dataflow_group_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    parent_application uuid;
    ancestor uuid;
    depth integer := 0;
BEGIN
    IF TG_OP = 'UPDATE' AND NEW.id IS DISTINCT FROM OLD.id THEN
        RAISE EXCEPTION 'Dataflow group identity is immutable' USING ERRCODE = '23514';
    END IF;
    IF NEW.parent_id IS NOT NULL AND NEW.parent_id = NEW.id THEN
        RAISE EXCEPTION 'Dataflow group cannot be its own parent' USING ERRCODE = '23514';
    END IF;
    IF NEW.parent_id IS NOT NULL THEN
        SELECT g.application_id INTO parent_application
        FROM inventory_dataflow_group g WHERE g.id = NEW.parent_id;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Dataflow group parent does not exist' USING ERRCODE = '23514';
        END IF;
        IF NEW.application_id IS DISTINCT FROM parent_application THEN
            RAISE EXCEPTION 'Dataflow group application must match its parent' USING ERRCODE = '23514';
        END IF;
        ancestor := NEW.parent_id;
        WHILE ancestor IS NOT NULL LOOP
            IF ancestor = NEW.id THEN
                RAISE EXCEPTION 'Dataflow group hierarchy would contain a cycle' USING ERRCODE = '23514';
            END IF;
            depth := depth + 1;
            IF depth > 64 THEN
                RAISE EXCEPTION 'Dataflow group hierarchy depth exceeds 64' USING ERRCODE = '23514';
            END IF;
            SELECT g.parent_id INTO ancestor FROM inventory_dataflow_group g WHERE g.id = ancestor;
            IF NOT FOUND THEN
                RAISE EXCEPTION 'Dataflow group ancestor does not exist' USING ERRCODE = '23514';
            END IF;
        END LOOP;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.application_id IS DISTINCT FROM OLD.application_id THEN
        IF EXISTS (SELECT 1 FROM inventory_dataflow_group c
                   WHERE c.parent_id = OLD.id AND c.application_id IS DISTINCT FROM NEW.application_id)
           OR EXISTS (SELECT 1 FROM inventory_dataflow f
                      WHERE f.group_id = OLD.id AND f.application_id IS DISTINCT FROM NEW.application_id) THEN
            RAISE EXCEPTION 'Dataflow group application change conflicts with children or flows'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION inventory_dataflow_flow_consistency() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE group_application uuid;
BEGIN
    IF NEW.group_id IS NOT NULL THEN
        SELECT g.application_id INTO group_application
        FROM inventory_dataflow_group g WHERE g.id = NEW.group_id;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Dataflow group does not exist' USING ERRCODE = '23514';
        END IF;
        IF NEW.application_id IS DISTINCT FROM group_application THEN
            RAISE EXCEPTION 'Dataflow application must match its group' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER inventory_dataflow_group_catalog_lock
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON inventory_dataflow_group
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_dataflow_catalog_lock();
CREATE TRIGGER inventory_dataflow_catalog_lock
    BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON inventory_dataflow
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_dataflow_catalog_lock();
CREATE TRIGGER inventory_dataflow_group_hierarchy_guard
    BEFORE INSERT OR UPDATE ON inventory_dataflow_group
    FOR EACH ROW EXECUTE FUNCTION inventory_dataflow_group_guard();
CREATE TRIGGER inventory_dataflow_group_consistency
    BEFORE INSERT OR UPDATE ON inventory_dataflow
    FOR EACH ROW EXECUTE FUNCTION inventory_dataflow_flow_consistency();

-- Compact content-type tokens keep the established `inventory.<model>`
-- vocabulary while native tables stay snake_case. This is the single mapping
-- used by tag scoping and custom-field subject discovery; there is no alias.
CREATE FUNCTION inventory_content_type_table(app_label text, model text)
RETURNS text LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE
        WHEN app_label = 'inventory' AND model = 'applicationrole'
            THEN 'inventory_application_role'
        WHEN app_label = 'inventory' AND model = 'dataflowgroup'
            THEN 'inventory_dataflow_group'
        WHEN app_label = 'inventory' AND model = 'addressgroup'
            THEN 'inventory_address_group'
        ELSE app_label || '_' || model
    END
$$;

CREATE OR REPLACE FUNCTION inventory_tag_scope_type_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM inventory_content_type c
        WHERE c.id=NEW.content_type_id
          AND c.app_label ~ '^[a-z][a-z0-9]*$' AND c.model ~ '^[a-z][a-z0-9]*$'
          AND inventory_content_type_table(c.app_label, c.model) IN (SELECT table_name FROM inventory_tag_subjects())
    ) THEN
        RAISE EXCEPTION 'Tag object type is not a registered taggable target' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION inventory_tag_membership_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM inventory_validate_tag_ids(NEW.tags);
    IF EXISTS (
        SELECT 1 FROM unnest(NEW.tags) token
        WHERE EXISTS (SELECT 1 FROM inventory_tag_object_type s WHERE s.tag_id=token::uuid)
          AND NOT EXISTS (
              SELECT 1 FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id
              WHERE s.tag_id=token::uuid AND inventory_content_type_table(c.app_label, c.model)=TG_TABLE_NAME
          )
    ) THEN
        RAISE EXCEPTION 'Tag is incompatible with inventory object type' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION inventory_tag_scope_check(tag uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE subject record; referenced boolean;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM inventory_tag_object_type WHERE tag_id=tag) THEN RETURN; END IF;
    FOR subject IN SELECT table_name FROM inventory_tag_subjects() LOOP
        IF NOT EXISTS (
            SELECT 1 FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id
            WHERE s.tag_id=tag AND inventory_content_type_table(c.app_label, c.model)=subject.table_name
        ) THEN
            EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%I WHERE tags @> ARRAY[$1])', subject.table_name)
                INTO referenced USING tag::text;
            IF referenced THEN
                RAISE EXCEPTION 'Tag scope conflicts with existing inventory membership' USING ERRCODE='23514';
            END IF;
        END IF;
    END LOOP;
END
$$;

-- The native catalogue now also recognizes the new taggable owners. Existing
-- owners keep their installation triggers; only new columns get new triggers.
CREATE OR REPLACE FUNCTION inventory_tag_subjects() RETURNS TABLE(table_name text)
LANGUAGE sql STABLE AS $$
    SELECT DISTINCT r.relname::text
    FROM pg_class r JOIN pg_namespace n ON n.oid=r.relnamespace
    JOIN pg_attribute a ON a.attrelid=r.oid
    WHERE n.nspname='public' AND r.relkind IN ('r','p')
      AND (r.relname ~ '^(circuits|dcim|ipam|tenancy|virtualization|vpn|wireless)_'
           OR r.relname ~ '^inventory_(application|application_role|dataflow_group|address_group|dataflow)$')
      AND a.attname='tags' AND a.atttypid='text[]'::regtype AND NOT a.attisdropped
    ORDER BY 1
$$;

CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON inventory_application
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON inventory_application
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON inventory_dataflow_group
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON inventory_dataflow_group
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON inventory_address_group
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON inventory_address_group
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON inventory_dataflow
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON inventory_dataflow
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();

INSERT INTO inventory_content_type (app_label, model) VALUES
    ('inventory', 'applicationrole'),
    ('inventory', 'application'),
    ('inventory', 'dataflowgroup'),
    ('inventory', 'addressgroup'),
    ('inventory', 'dataflow')
ON CONFLICT (app_label, model) DO NOTHING;

-- Contact assignments use a typed generic reference. Serialize both writers
-- with the existing contact graph and check the final state, including replay.
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON inventory_application
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();

CREATE FUNCTION inventory_validate_application_contacts() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM tenancy_contactassignment a
        JOIN inventory_content_type ct ON ct.id = a.object_type_id
        LEFT JOIN inventory_application p ON p.id = a.object_id
        WHERE ct.app_label = 'inventory' AND ct.model = 'application' AND p.id IS NULL
    ) THEN
        RAISE EXCEPTION 'Application contact assignment requires an existing application' USING ERRCODE = '23503';
    END IF;
END $$;
SELECT inventory_validate_application_contacts();

CREATE FUNCTION inventory_check_application_contacts() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM inventory_validate_application_contacts();
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER inventory_application_contacts
    AFTER INSERT OR UPDATE OR DELETE ON inventory_application
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION inventory_check_application_contacts();
CREATE CONSTRAINT TRIGGER inventory_application_contact_target
    AFTER INSERT OR UPDATE OR DELETE ON tenancy_contactassignment
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION inventory_check_application_contacts();
