import { describe, expect, it } from "vitest";
import { fieldsByTab, tabConfig } from "./components/inventory/inventoryTypes";
import {
  CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY,
  customFieldContentTypeTokenFor,
  inventoryCustomFieldColumnKey,
  inventoryCustomFieldDataColumns,
  inventoryColumnKeyForField,
  inventoryFieldRendersAsDataColumn,
  inventoryNameColumnKey,
  inventoryTableConfigFor,
  inventoryTableConfigWithCustomFields,
} from "./tableConfig";
import type { InventoryFilterColumn } from "./tableConfig";
import { buildInventoryTableParams } from "./api";
import { inventoryEntityHasStatus, type InventoryEntityKey } from "@/shared/api/types";
import { sanitizeDataTableColumnFilters } from "@/shared/ui-shared/tableState";
import type {
  ChoiceSet,
  ContentType,
  CustomFieldDefinition,
} from "@/features/inventory/custom-fields-api";

const byColKey = (cols: InventoryFilterColumn[]) =>
  Object.fromEntries(cols.map((c) => [c.colKey, c]));

const contentTypes = [
  {
    id: "ct-site",
    app_label: "dcim",
    model: "site",
    label: "Site (dcim)",
    supported_for_custom_fields: true,
    token: CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY.Sites,
  },
  {
    id: "ct-device",
    app_label: "dcim",
    model: "device",
    label: "Device (dcim)",
    supported_for_custom_fields: true,
    token: CUSTOM_FIELD_CONTENT_TYPE_TOKEN_BY_ENTITY.Devices,
  },
] satisfies ContentType[];

const choiceSets = [
  {
    id: "choices-priority",
    name: "Priority",
    description: "",
    extra_choices: [
      { value: "low", label: "Low" },
      { value: "high", label: "High" },
    ],
    choice_colors: {},
    order_alphabetically: false,
    created_at: "2026-07-06T00:00:00Z",
    updated_at: "2026-07-06T00:00:00Z",
  },
] satisfies ChoiceSet[];

function definition(
  name: string,
  fieldType: string,
  overrides: Partial<CustomFieldDefinition> = {},
): CustomFieldDefinition {
  return {
    id: `def-${name}`,
    name,
    label: name,
    field_type: fieldType,
    description: "",
    group_name: "",
    required: false,
    enforce_unique: false,
    is_secret: false,
    default_value: null,
    weight: 0,
    validation_minimum: null,
    validation_maximum: null,
    validation_regex: "",
    validation_schema: null,
    choice_set_id: null,
    related_object_type_id: null,
    related_object_filter: null,
    ui_visible: "always",
    ui_editable: "yes",
    is_cloneable: true,
    search_weight: 0,
    filter_logic: "loose",
    object_types: ["ct-site"],
    created_at: "2026-07-06T00:00:00Z",
    updated_at: "2026-07-06T00:00:00Z",
    ...overrides,
  };
}

function renderedColumnKeys(entity: InventoryEntityKey): Set<string> {
  const keys = new Set<string>([inventoryNameColumnKey(entity)]);
  if (inventoryEntityHasStatus(entity)) keys.add("status");
  if (entity === "Prefixes") keys.add("utilization");
  for (const field of fieldsByTab[entity] ?? []) {
    if (inventoryFieldRendersAsDataColumn(entity, field)) {
      keys.add(inventoryColumnKeyForField(entity, field));
    }
  }
  return keys;
}

describe("inventoryTableConfigFor", () => {
  it("renders the VM type association from the shared form-field registry", () => {
    expect(renderedColumnKeys("Virtual Machines")).toContain("virtual_machine_type");
    expect(inventoryTableConfigFor("Virtual Machines").sortKeys).toContain("virtual_machine_type");
    expect(renderedColumnKeys("Virtual Machine Types")).toEqual(new Set([
      "name", "slug", "default_platform", "default_vcpus", "default_memory", "owner",
    ]));
  });
  it("declares sort coverage for every inventory tab rendered in the resource table", () => {
    const missing = tabConfig
      .map((tab) => tab.key)
      .filter((entity) => inventoryTableConfigFor(entity).sortKeys.length === 0);

    expect(missing).toEqual([]);
  });

  it("keeps every filter column sortable or explicitly declared filter-only", () => {
    const unsupportedFilters = tabConfig.flatMap((tab) => {
      const config = inventoryTableConfigFor(tab.key);
      const sortKeys = new Set(config.sortKeys);
      return config.filterColumns
        .filter((column) => !sortKeys.has(column.colKey) && !config.unsortableKeys?.includes(column.colKey))
        .map((column) => `${tab.key}.${column.colKey}`);
    });

    expect(unsupportedFilters).toEqual([]);
  });

  it("keeps every filter column bound to a rendered table column key", () => {
    const detachedFilters = tabConfig.flatMap((tab) => {
      const rendered = renderedColumnKeys(tab.key);
      return inventoryTableConfigFor(tab.key).filterColumns
        .filter((column) => !rendered.has(column.colKey))
        .map((column) => `${tab.key}.${column.colKey}`);
    });

    expect(detachedFilters).toEqual([]);
  });

  it("keeps filter query params unique per entity", () => {
    const duplicateParams = tabConfig.flatMap((tab) => {
      const seen = new Set<string>();
      const duplicates = new Set<string>();
      for (const column of inventoryTableConfigFor(tab.key).filterColumns) {
        if (seen.has(column.param)) duplicates.add(column.param);
        else seen.add(column.param);
      }
      return Array.from(duplicates).map((param) => `${tab.key}.${param}`);
    });

    expect(duplicateParams).toEqual([]);
  });

  it("keeps FK filter option sources inside the inventory entity catalog", () => {
    const entities = new Set(tabConfig.map((tab) => tab.key));
    const missingSources = tabConfig.flatMap((tab) =>
      inventoryTableConfigFor(tab.key).filterColumns.flatMap((column) => {
        if (column.type !== "fk") return [];
        const sources = Array.isArray(column.fkEntity) ? column.fkEntity : [column.fkEntity];
        return sources
          .filter((source) => !entities.has(source))
          .map((source) => `${tab.key}.${column.colKey} -> ${source}`);
      }),
    );

    expect(missingSources).toEqual([]);
  });

  it("keeps enum filter options explicit and non-empty", () => {
    const invalidEnums = tabConfig.flatMap((tab) =>
      inventoryTableConfigFor(tab.key).filterColumns
        .filter((column) => column.type === "enum")
        .filter((column) => column.enumValues.length === 0)
        .map((column) => `${tab.key}.${column.colKey}`),
    );

    expect(invalidEnums).toEqual([]);
  });

  it("keeps filter query params distinct from rendered column keys unless intentionally equal", () => {
    const missingParams = tabConfig.flatMap((tab) =>
      inventoryTableConfigFor(tab.key).filterColumns
        .filter((column) => column.param.trim().length === 0)
        .map((column) => `${tab.key}.${column.colKey}`),
    );

    expect(missingParams).toEqual([]);
  });

  it("does not render identity fields again as data columns", () => {
    const collisions = tabConfig.flatMap((tab) => {
      const entity = tab.key;
      const identityKey = inventoryNameColumnKey(entity);
      const fields = fieldsByTab[entity] ?? [];

      return fields
        .filter((field) => field.type !== "textarea" && field.type !== "icon")
        .filter((field) => !(entity === "Prefixes" && field.key === "prefix"))
        .filter((field) => field.key !== "name")
        .filter((field) => inventoryFieldRendersAsDataColumn(entity, field))
        .filter((field) => inventoryColumnKeyForField(entity, field) === identityKey)
        .map((field) => `${entity}.${String(field.key)} -> ${identityKey}`);
    });

    expect(collisions).toEqual([]);
  });

  it("exposes the locked device sort keys (matching the backend allowlist)", () => {
    const config = inventoryTableConfigFor("Devices");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "site",
      "role",
      "device_type",
      "platform",
      "tenant",
      "rack",
      "primary_ip",
      "serial",
      "asset_tag",
      "position",
      "face",
      "airflow",
    ]);
  });

  it("only declares device filter columns the backend accepts", () => {
    const config = inventoryTableConfigFor("Devices");
    const byKey = byColKey(config.filterColumns);

    // status is a static enum filter sent under the `status` param.
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect((byKey.status as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["active", "planned", "staged", "decommissioning", "offline"]);

    // FK filters bind to the exact *_id params list_devices accepts.
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.role).toMatchObject({ param: "role_id", type: "fk", fkEntity: "Device Roles" });
    expect(byKey.device_type).toMatchObject({ param: "device_type_id", type: "fk", fkEntity: "Device Types" });
    expect(byKey.platform).toMatchObject({ param: "platform_id", type: "fk", fkEntity: "Platforms" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(byKey.rack).toMatchObject({ param: "rack_id", type: "fk", fkEntity: "Racks" });
  });

  it("renames the deviceType column to the backend device_type sort key", () => {
    const config = inventoryTableConfigFor("Devices");
    expect(config.columnKeyOverrides).toEqual({
      deviceType: "device_type",
      rackName: "rack",
      primaryIp: "primary_ip",
      assetTag: "asset_tag",
    });
  });

  it("matches the site_sort_column allowlist and SiteListQuery filter params", () => {
    const config = inventoryTableConfigFor("Sites");
    // slug is in the backend allowlist but has no rendered column → dropped.
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "region",
      "site_group",
      "tenant",
      "facility",
      "time_zone",
      "latitude",
      "longitude",
    ]);
    expect(config.columnKeyOverrides).toEqual({ siteGroupName: "site_group", timeZone: "time_zone" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.region).toMatchObject({ param: "region_id", type: "fk", fkEntity: "Regions" });
    expect(byKey.site_group).toMatchObject({ param: "site_group_id", type: "fk", fkEntity: "Site Groups" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the cluster_sort_column allowlist and ClusterListQuery filter params", () => {
    const config = inventoryTableConfigFor("Clusters");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "cluster_type",
      "cluster_group",
      "site",
      "tenant",
      "description",
      "comments",
    ]);
    expect(config.columnKeyOverrides).toEqual({
      clusterTypeName: "cluster_type",
      clusterGroupName: "cluster_group",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.cluster_type).toMatchObject({ param: "cluster_type_id", type: "fk", fkEntity: "Cluster Types" });
    expect(byKey.cluster_group).toMatchObject({ param: "cluster_group_id", type: "fk", fkEntity: "Cluster Groups" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("exposes custom-field filters only for backend-supported entity lists", () => {
    expect(customFieldContentTypeTokenFor("Sites")).toBe("dcim.site");
    expect(customFieldContentTypeTokenFor("Devices")).toBe("dcim.device");
    expect(customFieldContentTypeTokenFor("Clusters")).toBe("virtualization.cluster");
    expect(customFieldContentTypeTokenFor("Tenants")).toBe("tenancy.tenant");
    expect(customFieldContentTypeTokenFor("Services")).toBe("ipam.service");
    expect(customFieldContentTypeTokenFor("VM Interfaces")).toBe("virtualization.vminterface");
    expect(customFieldContentTypeTokenFor("Inventory Items")).toBe("dcim.inventoryitem");

    const base = inventoryTableConfigFor("Tenants");
    const config = inventoryTableConfigWithCustomFields(
      base,
      "Tenants",
      [definition("flag", "boolean")],
      choiceSets,
      contentTypes,
    );
    expect(config).toBe(base);
    expect(config.filterColumns.some((column) => column.colKey.startsWith("cf_"))).toBe(false);
  });

  it("builds safe option-style custom-field filters and omits unsupported controls", () => {
    const config = inventoryTableConfigWithCustomFields(
      inventoryTableConfigFor("Sites"),
      "Sites",
      [
        definition("enabled", "boolean"),
        definition("priority", "select", { choice_set_id: "choices-priority" }),
        definition("owner", "object", { related_object_type_id: "ct-device" }),
        definition("token", "text", { is_secret: true }),
        definition("payload", "json"),
        definition("disabled_flag", "boolean", { filter_logic: "disabled" }),
        definition("title", "text"),
      ],
      choiceSets,
      contentTypes,
    );
    const dynamic = byColKey(config.filterColumns);

    expect(dynamic.cf_enabled).toMatchObject({ param: "cf_enabled", type: "enum" });
    expect(dynamic.cf_priority).toMatchObject({ param: "cf_priority", type: "enum" });
    expect(dynamic.cf_owner).toMatchObject({ param: "cf_owner", type: "fk", fkEntity: "Devices" });
    expect(dynamic.cf_token).toMatchObject({ param: "cf_token__present", type: "enum" });
    expect(dynamic.cf_payload).toBeUndefined();
    expect(dynamic.cf_disabled_flag).toBeUndefined();
    expect(dynamic.cf_title).toBeUndefined();
  });

  it("keeps secret custom fields out of data columns", () => {
    const visible = definition("visible_text", "text");
    const secret = definition("token", "text", { is_secret: true });
    const hidden = definition("hidden_text", "text", { ui_visible: "hidden" });

    expect(inventoryCustomFieldDataColumns([visible, secret, hidden])).toEqual([visible]);
    expect(inventoryCustomFieldColumnKey(visible)).toBe("cf_visible_text");
  });

  it("sanitizes stale custom-field filters before table dispatch", () => {
    const config = inventoryTableConfigWithCustomFields(
      inventoryTableConfigFor("Sites"),
      "Sites",
      [definition("token", "text", { is_secret: true })],
      choiceSets,
      contentTypes,
    );
    const allowedKeys = config.filterColumns.map((column) => column.colKey);

    expect(
      sanitizeDataTableColumnFilters(
        {
          cf_token__present: ["cleartext-secret"],
          cf_removed: ["stale"],
          cf_token: ["true"],
        },
        allowedKeys,
      ),
    ).toEqual({ cf_token: ["true"] });
  });

  it("serializes secret filters as presence-only query params", () => {
    const config = inventoryTableConfigWithCustomFields(
      inventoryTableConfigFor("Sites"),
      "Sites",
      [definition("token", "text", { is_secret: true })],
      choiceSets,
      contentTypes,
    );
    const tokenFilter = byColKey(config.filterColumns).cf_token;

    expect(tokenFilter).toMatchObject({ param: "cf_token__present" });
    expect(tokenFilter?.type).toBe("enum");
    if (!tokenFilter || tokenFilter.type !== "enum") {
      throw new Error("expected secret custom-field filter to be an enum");
    }
    expect(tokenFilter.enumValues.map((value) => value.value)).toEqual(["true", "false"]);
    expect(
      buildInventoryTableParams({
        limit: 25,
        offset: 0,
        filters: { [tokenFilter.param]: ["true"] },
      }),
    ).toEqual({
      limit: 25,
      offset: 0,
      cf_token__present: "true",
    });
  });

  it("matches the device_type_sort_column allowlist (name column renders model)", () => {
    const config = inventoryTableConfigFor("Device Types");
    expect(config.sortKeys).toEqual(["model", "manufacturer"]);
    expect(config.identity).toEqual({ key: "model", label: "Model" });
    expect(config.columnKeyOverrides).toEqual({ manufacturerName: "manufacturer" });
    expect(byColKey(config.filterColumns).manufacturer)
      .toMatchObject({ param: "manufacturer_id", type: "fk", fkEntity: "Manufacturers" });
  });

  it("matches the rack_sort_column allowlist and RackListQuery filter params", () => {
    const config = inventoryTableConfigFor("Racks");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "site",
      "tenant",
      "rack_role",
      "rack_type",
      "facility_id",
      "serial",
      "asset_tag",
      "width",
      "u_height",
      "outer_width",
      "outer_depth",
      "desc_units",
    ]);
    expect(config.columnKeyOverrides).toEqual({
      rackRoleName: "rack_role",
      rackType: "rack_type",
      facilityId: "facility_id",
      assetTag: "asset_tag",
      uHeight: "u_height",
      outerWidth: "outer_width",
      outerDepth: "outer_depth",
      descUnits: "desc_units",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    // role_id binds the post-override rack_role column key.
    expect(byKey.rack_role).toMatchObject({ param: "role_id", type: "fk", fkEntity: "Rack Roles" });
    expect(byKey.desc_units).toMatchObject({ param: "desc_units", type: "enum" });
    // location_id has no rendered column → omitted.
    expect(config.filterColumns.some((c) => c.param === "location_id")).toBe(false);
  });

  it("matches the prefix_sort_column allowlist and PrefixListQuery filter params", () => {
    const config = inventoryTableConfigFor("Prefixes");
    expect(config.sortKeys).toEqual(["prefix", "status", "site", "vrf", "tenant", "is_pool", "mark_utilized", "utilization"]);
    expect(config.identity).toEqual({ key: "prefix", label: "inventory.resources.fields.prefix", fieldKey: "prefix" });
    expect(config.columnKeyOverrides).toEqual({ vrfName: "vrf", isPool: "is_pool", markUtilized: "mark_utilized" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.vrf).toMatchObject({ param: "vrf_id", type: "fk", fkEntity: "VRFs" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(byKey.is_pool).toMatchObject({ param: "is_pool", type: "enum" });
    expect(byKey.mark_utilized).toMatchObject({ param: "mark_utilized", type: "enum" });
    // prefix is a free-text identity column (sort only); role_id has no column.
    expect(byKey.prefix).toBeUndefined();
    expect(config.filterColumns.some((c) => c.param === "role_id")).toBe(false);
  });

  it("matches the vlan_sort_column allowlist and VlanListQuery filter params", () => {
    const config = inventoryTableConfigFor("VLANs");
    expect(config.sortKeys).toEqual(["vid", "name", "status", "site", "tenant"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    // vid is an exact-int filter (not a multi-value IN checkbox) and group_id has
    // no rendered column → both omitted; vid/name stay sort-only.
    expect(byKey.vid).toBeUndefined();
    expect(byKey.name).toBeUndefined();
    expect(config.filterColumns.some((c) => c.param === "group_id")).toBe(false);
  });

  it("matches the ip_address_sort_column allowlist and representable IpAddressListQuery filter params", () => {
    const config = inventoryTableConfigFor("IP Addresses");
    expect(config.sortKeys).toEqual([
      "address",
      "status",
      "role",
      "dns_name",
      "description",
      "assigned_object_id",
      "assigned_object_type_id",
      "tenant_id",
      "vrf_id",
    ]);
    expect(config.identity).toEqual({ key: "address", label: "inventory.resources.fields.ipAddress", fieldKey: "prefix" });
    expect(config.columnKeyOverrides).toEqual({
      prefix: "address",
      dnsName: "dns_name",
      interfaceName: "assigned_object_id",
      tenant: "tenant_id",
      vrfName: "vrf_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.assigned_object_id).toMatchObject({
      param: "assigned_object_id",
      type: "fk",
      fkEntity: ["Interfaces", "VM Interfaces"],
    });
    expect(byKey.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(byKey.vrf_id).toMatchObject({ param: "vrf_id", type: "fk", fkEntity: "VRFs" });
    expect(byKey.role).toBeUndefined();
    expect(byKey.assigned_object_type_id).toBeUndefined();
  });

  it("normalizes the IP-address identity field to the rendered address column key", () => {
    const ipAddressField = fieldsByTab["IP Addresses"]?.find((field) => field.key === "prefix");

    expect(ipAddressField).toBeDefined();
    expect(inventoryColumnKeyForField("IP Addresses", ipAddressField!)).toBe("address");
    expect(inventoryNameColumnKey("IP Addresses")).toBe("address");
  });

  it("matches the virtual_machine_sort_column allowlist and filter params", () => {
    const config = inventoryTableConfigFor("Virtual Machines");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "site",
      "cluster",
      "role",
      "platform",
      "virtual_machine_type",
      "tenant",
      "primary_ip",
      "vcpus",
      "memory",
      "disk",
    ]);
    expect(config.columnKeyOverrides).toEqual({ primaryIp: "primary_ip", virtualMachineType: "virtual_machine_type" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.cluster).toMatchObject({ param: "cluster_id", type: "fk", fkEntity: "Clusters" });
    expect(byKey.role).toMatchObject({ param: "role_id", type: "fk", fkEntity: "Device Roles" });
    expect(byKey.platform).toMatchObject({ param: "platform_id", type: "fk", fkEntity: "Platforms" });
    expect(byKey.virtual_machine_type).toMatchObject({ param: "virtual_machine_type_id", type: "fk", fkEntity: "Virtual Machine Types" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("declares VPN tunnel filters accepted by TunnelListQuery", () => {
    const config = inventoryTableConfigFor("Tunnels");
    const byKey = byColKey(config.filterColumns);

    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "encapsulation",
      "description",
      "comments",
      "group_id",
      "ipsec_profile_id",
      "tunnel_id",
      "tenant_id",
    ]);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.encapsulation).toMatchObject({ param: "encapsulation", type: "enum" });
    expect(byKey.group_id).toMatchObject({ param: "group_id", type: "fk", fkEntity: "Tunnel Groups" });
    expect(byKey.ipsec_profile_id).toMatchObject({ param: "ipsec_profile_id", type: "fk", fkEntity: "IPsec Profiles" });
    expect(byKey.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("declares VPN policy/proposal filters accepted by their list queries", () => {
    expect(byColKey(inventoryTableConfigFor("IKE Policies").filterColumns).mode)
      .toMatchObject({ param: "mode", type: "enum" });

    const ikeProposalFilters = byColKey(inventoryTableConfigFor("IKE Proposals").filterColumns);
    expect(ikeProposalFilters.authentication_method)
      .toMatchObject({ param: "authentication_method", type: "enum" });
    expect(ikeProposalFilters.encryption_algorithm)
      .toMatchObject({ param: "encryption_algorithm", type: "enum" });
    expect(ikeProposalFilters.authentication_algorithm)
      .toMatchObject({ param: "authentication_algorithm", type: "enum" });

    const ipsecProposalFilters = byColKey(inventoryTableConfigFor("IPsec Proposals").filterColumns);
    expect(ipsecProposalFilters.encryption_algorithm)
      .toMatchObject({ param: "encryption_algorithm", type: "enum" });
    expect(ipsecProposalFilters.authentication_algorithm)
      .toMatchObject({ param: "authentication_algorithm", type: "enum" });
  });

  it("declares VPN profile and termination FK filters accepted by list queries", () => {
    const profileFilters = byColKey(inventoryTableConfigFor("IPsec Profiles").filterColumns);
    expect(profileFilters.mode).toMatchObject({ param: "mode", type: "enum" });
    expect(profileFilters.ike_policy).toMatchObject({ param: "ike_policy_id", type: "fk", fkEntity: "IKE Policies" });
    expect(profileFilters.ipsec_policy).toMatchObject({ param: "ipsec_policy_id", type: "fk", fkEntity: "IPsec Policies" });

    const terminationFilters = byColKey(inventoryTableConfigFor("Tunnel Terminations").filterColumns);
    expect(terminationFilters.role).toMatchObject({ param: "role", type: "enum" });
    expect(terminationFilters.tunnel_id).toMatchObject({ param: "tunnel_id", type: "fk", fkEntity: "Tunnels" });
    expect(terminationFilters.termination_id).toMatchObject({
      param: "termination_id",
      type: "fk",
      fkEntity: ["Interfaces", "VM Interfaces"],
    });
    expect(terminationFilters.outside_ip_id).toMatchObject({ param: "outside_ip_id", type: "fk", fkEntity: "IP Addresses" });
    expect(terminationFilters.termination_type_id).toBeUndefined();
  });

  it("matches the region_sort_column allowlist and RegionListQuery filter params", () => {
    const config = inventoryTableConfigFor("Regions");
    expect(config.sortKeys).toEqual(["name", "parent"]);
    expect(config.columnKeyOverrides).toEqual({ parentName: "parent" });
    expect(byColKey(config.filterColumns).parent)
      .toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Regions" });
  });

  it("matches the tenant_sort_column allowlist and TenantListQuery filter params", () => {
    const config = inventoryTableConfigFor("Tenants");
    expect(config.sortKeys).toEqual(["name", "tenant_group"]);
    expect(config.columnKeyOverrides).toEqual({ groupName: "tenant_group" });
    expect(byColKey(config.filterColumns).tenant_group)
      .toMatchObject({ param: "group_id", type: "fk", fkEntity: "Tenant Groups" });
  });

  it("matches tenant group column metadata and TenantGroupListQuery filter params", () => {
    const config = inventoryTableConfigFor("Tenant Groups");
    expect(config.sortKeys).toEqual(["name", "slug", "parent_id", "description"]);
    expect(config.columnKeyOverrides).toEqual({ parentName: "parent_id" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.parent_id).toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Tenant Groups" });
  });

  it("matches site group column metadata and SiteGroupListQuery filter params", () => {
    const config = inventoryTableConfigFor("Site Groups");
    expect(config.sortKeys).toEqual(["name", "slug", "parent_id", "description"]);
    expect(config.columnKeyOverrides).toEqual({ parentName: "parent_id" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.parent_id).toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Site Groups" });
  });

  it("matches device role column metadata and DeviceRoleListQuery filter params", () => {
    const config = inventoryTableConfigFor("Device Roles");
    expect(config.sortKeys).toEqual(["external_id", "name", "slug", "color", "vm_role", "description", "tags", "piping_synced_at"]);
    expect(config.columnKeyOverrides).toEqual({ vmRole: "vm_role", pipingSyncedAt: "piping_synced_at" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.vm_role).toMatchObject({ param: "vm_role", type: "enum" });
    expect((byKey.vm_role as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["true", "false"]);
  });

  it("matches platform column metadata and PlatformListQuery filter params", () => {
    const config = inventoryTableConfigFor("Platforms");
    expect(config.sortKeys).toEqual(["name", "slug", "manufacturer", "napalm_driver", "description"]);
    expect(config.columnKeyOverrides).toEqual({
      manufacturerName: "manufacturer",
      napalmDriver: "napalm_driver",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.manufacturer).toMatchObject({ param: "manufacturer_id", type: "fk", fkEntity: "Manufacturers" });
  });

  it("matches the contact CONTACT_COLUMNS metadata and ContactListQuery filter params", () => {
    const config = inventoryTableConfigFor("Contacts");
    expect(config.sortKeys).toEqual(["name", "group", "title", "phone", "email", "address", "link", "description"]);
    expect(config.columnKeyOverrides).toEqual({ groupName: "group" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.group).toMatchObject({ param: "group_id", type: "fk", fkEntity: "Contact Groups" });
  });

  it("matches the contact_role CONTACT_ROLE_COLUMNS metadata (sort only)", () => {
    const config = inventoryTableConfigFor("Contact Roles");
    expect(config.sortKeys).toEqual(["name", "slug", "description"]);
    expect(config.filterColumns).toEqual([]);
  });

  it("matches the contact_group CONTACT_GROUP_COLUMNS metadata and filter params", () => {
    const config = inventoryTableConfigFor("Contact Groups");
    expect(config.sortKeys).toEqual(["name", "slug", "parent", "description"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.parent).toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Contact Groups" });
  });

  it("matches the contact_assignment CONTACT_ASSIGNMENT_COLUMNS metadata and filter params", () => {
    const config = inventoryTableConfigFor("Contact Assignments");
    expect(config.sortKeys).toEqual(["contentType", "objectId", "contact", "role", "priority"]);
    expect(config.identity).toEqual({
      key: "objectId",
      label: "inventory.resources.fields.assignedTo",
      fieldKey: "object",
    });
    expect(config.columnKeyOverrides).toEqual({ object: "objectId", contactName: "contact" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.contact).toMatchObject({ param: "contact_id", type: "fk", fkEntity: "Contacts" });
    expect(byKey.role).toMatchObject({ param: "role_id", type: "fk", fkEntity: "Contact Roles" });
    expect(byKey.priority).toMatchObject({ param: "priority", type: "enum" });
    expect((byKey.priority as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["primary", "secondary", "tertiary", "inactive"]);
    expect(byKey.contentType).toBeUndefined();
    expect(byKey.objectId).toBeUndefined();
  });

  it("matches the interface column metadata and InterfaceListQuery filter params", () => {
    const config = inventoryTableConfigFor("Interfaces");
    expect(config.sortKeys).toEqual(["device_id", "name", "label", "description", "type", "enabled", "mgmt_only"]);
    expect(config.columnKeyOverrides).toEqual({
      deviceName: "device_id",
      interfaceType: "type",
      mgmtOnly: "mgmt_only",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.device_id).toMatchObject({ param: "device_id", type: "fk", fkEntity: "Devices" });
    expect(byKey.type).toMatchObject({ param: "type", type: "enum" });
    expect(byKey.enabled).toMatchObject({ param: "enabled", type: "enum" });
    expect(byKey.mgmt_only).toMatchObject({ param: "mgmt_only", type: "enum" });
    expect((byKey.enabled as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["true", "false"]);
    expect((byKey.type as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["virtual", "other"]);
  });

  it("matches the vm_interface column metadata and VmInterfaceListQuery filter params", () => {
    const config = inventoryTableConfigFor("VM Interfaces");
    expect(config.sortKeys).toEqual(["virtual_machine_id", "name", "enabled", "mtu", "mode", "description"]);
    expect(config.columnKeyOverrides).toEqual({
      virtualMachineName: "virtual_machine_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.virtual_machine_id).toMatchObject({
      param: "virtual_machine_id",
      type: "fk",
      fkEntity: "Virtual Machines",
    });
    expect(byKey.enabled).toMatchObject({ param: "enabled", type: "enum" });
    expect(byKey.mode).toMatchObject({ param: "mode", type: "enum" });
    expect((byKey.enabled as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["true", "false"]);
    expect((byKey.mode as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["access", "tagged", "tagged-all"]);
  });

  it("matches the power_panel column metadata and PowerPanelListQuery filter params", () => {
    const config = inventoryTableConfigFor("Power Panels");
    expect(config.sortKeys).toEqual(["name", "site_id", "location_id", "description", "comments"]);
    expect(config.columnKeyOverrides).toEqual({
      site: "site_id",
      parentName: "location_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.site_id).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.location_id).toMatchObject({ param: "location_id", type: "fk", fkEntity: "Locations" });
  });

  it("matches the power_feed column metadata and PowerFeedListQuery filter params", () => {
    const config = inventoryTableConfigFor("Power Feeds");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "type",
      "supply",
      "phase",
      "voltage",
      "amperage",
      "max_utilization",
      "available_power",
      "power_panel_id",
      "rack_id",
      "mark_connected",
      "description",
      "comments",
      "tenant_id",
    ]);
    expect(config.columnKeyOverrides).toEqual({
      rackType: "type",
      powerPanelName: "power_panel_id",
      rackName: "rack_id",
      maxUtilization: "max_utilization",
      markConnected: "mark_connected",
      tenant: "tenant_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.type).toMatchObject({ param: "type", type: "enum" });
    expect(byKey.supply).toMatchObject({ param: "supply", type: "enum" });
    expect(byKey.phase).toMatchObject({ param: "phase", type: "enum" });
    expect(byKey.power_panel_id).toMatchObject({ param: "power_panel_id", type: "fk", fkEntity: "Power Panels" });
    expect(byKey.rack_id).toMatchObject({ param: "rack_id", type: "fk", fkEntity: "Racks" });
    expect(byKey.mark_connected).toMatchObject({ param: "mark_connected", type: "enum" });
    expect(byKey.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect((byKey.type as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["primary", "redundant"]);
    expect((byKey.supply as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["ac", "dc"]);
    expect((byKey.phase as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["single-phase", "three-phase"]);
  });

  it("matches the aggregate_sort_column allowlist and AggregateListQuery filter params", () => {
    const config = inventoryTableConfigFor("Aggregates");
    expect(config.sortKeys).toEqual(["prefix", "rir", "rirName", "tenant", "tenantName", "date_added", "description", "comments"]);
    expect(config.identity).toEqual({ key: "prefix", label: "inventory.resources.fields.prefix", fieldKey: "prefixCidr" });
    expect(config.columnKeyOverrides).toEqual({ prefixCidr: "prefix", dateAdded: "date_added" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.rirName).toMatchObject({ param: "rir_id", type: "fk", fkEntity: "RIRs" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the asn_sort_column allowlist and AsnListQuery filter params", () => {
    const config = inventoryTableConfigFor("ASNs");
    expect(config.sortKeys).toEqual(["asn", "rir", "rirName", "tenant", "tenantName", "description", "comments"]);
    expect(config.identity).toEqual({ key: "asn", label: "inventory.resources.fields.asn", fieldKey: "asn" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.rirName).toMatchObject({ param: "rir_id", type: "fk", fkEntity: "RIRs" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the vrf_sort_column allowlist and VrfListQuery filter params", () => {
    const config = inventoryTableConfigFor("VRFs");
    expect(config.sortKeys).toEqual(["name", "rd", "enforce_unique", "description", "comments", "tenant", "tenantName"]);
    expect(config.columnKeyOverrides).toEqual({ enforceUnique: "enforce_unique" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.enforce_unique).toMatchObject({ param: "enforce_unique", type: "enum" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the route_target_sort_column allowlist and RouteTargetListQuery filter params", () => {
    const config = inventoryTableConfigFor("Route Targets");
    expect(config.sortKeys).toEqual(["name", "description", "tenant", "tenantName"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the rir_sort_column allowlist and RirListQuery filter params", () => {
    const config = inventoryTableConfigFor("RIRs");
    expect(config.sortKeys).toEqual(["name", "slug", "is_private", "description", "owner"]);
    expect(config.columnKeyOverrides).toEqual({ isPrivate: "is_private" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.is_private).toMatchObject({ param: "is_private", type: "enum" });
    expect(byKey.owner).toBeUndefined();
  });

  it("matches the circuit_group_sort_column allowlist and CircuitGroupListQuery filter params", () => {
    const config = inventoryTableConfigFor("Circuit Groups");
    expect(config.sortKeys).toEqual(["name", "slug", "tenant", "description", "comments"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the cable column metadata and CableListQuery filter params", () => {
    const config = inventoryTableConfigFor("Cables");
    expect(config.sortKeys).toEqual(["type", "status", "label", "color", "length", "length_unit"]);
    expect(config.identity).toEqual({ key: "label", label: "Label", fieldKey: "label" });
    expect(config.columnKeyOverrides).toEqual({
      interfaceType: "type",
      lengthUnit: "length_unit",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.type).toMatchObject({ param: "type", type: "enum" });
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.length_unit).toMatchObject({ param: "length_unit", type: "enum" });
    expect(byKey.a_termination_type).toBeUndefined();
    expect(byKey.b_termination_type).toBeUndefined();
  });

  it("matches the circuit termination column metadata and CircuitTerminationListQuery filter params", () => {
    const config = inventoryTableConfigFor("Circuit Terminations");
    expect(config.sortKeys).toEqual([
      "term_side",
      "circuit_id",
      "xconnect_id",
      "pp_info",
      "port_speed",
      "upstream_speed",
      "mark_connected",
      "description",
    ]);
    expect(config.identity).toEqual({
      key: "circuit_id",
      label: "inventory.resources.fields.circuit",
      fieldKey: "circuitName",
    });
    expect(config.columnKeyOverrides).toEqual({
      termSide: "term_side",
      circuitName: "circuit_id",
      xconnectId: "xconnect_id",
      ppInfo: "pp_info",
      portSpeed: "port_speed",
      upstreamSpeed: "upstream_speed",
      markConnected: "mark_connected",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.term_side).toMatchObject({ param: "term_side", type: "enum" });
    expect(byKey.circuit_id).toMatchObject({ param: "circuit_id", type: "fk", fkEntity: "Circuits" });
    expect(byKey.mark_connected).toMatchObject({ param: "mark_connected", type: "enum" });
  });

  it("matches virtual circuit type and hydrated virtual circuit table metadata", () => {
    expect(inventoryTableConfigFor("Virtual Circuit Types").sortKeys).toEqual([
      "name",
      "slug",
      "color",
      "description",
      "comments",
    ]);

    const config = inventoryTableConfigFor("Virtual Circuits");
    expect(config.sortKeys).toEqual([
      "cid",
      "status",
      "provider_network",
      "virtual_circuit_type",
      "provider_account",
      "tenant",
      "description",
      "comments",
    ]);
    expect(config.identity).toEqual({ key: "cid", label: "CID" });
    expect(config.columnKeyOverrides).toEqual({
      providerNetworkName: "provider_network",
      virtualCircuitTypeName: "virtual_circuit_type",
      providerName: "provider_account",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.provider_network).toMatchObject({ param: "provider_network_id", type: "fk", fkEntity: "Provider Networks" });
    expect(byKey.virtual_circuit_type).toMatchObject({ param: "type_id", type: "fk", fkEntity: "Virtual Circuit Types" });
    expect(byKey.provider_account).toMatchObject({ param: "provider_account_id", type: "fk", fkEntity: "Provider Accounts" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the ip_range_sort_column allowlist and IpRangeListQuery filter params", () => {
    const config = inventoryTableConfigFor("IP Ranges");
    expect(config.sortKeys).toEqual([
      "start_address",
      "end_address",
      "size",
      "status",
      "vrf",
      "vrfName",
      "tenant",
      "tenantName",
      "description",
      "mark_utilized",
      "mark_populated",
    ]);
    expect(config.identity).toEqual({
      key: "start_address",
      label: "inventory.resources.fields.startAddress",
      fieldKey: "startAddress",
    });
    expect(config.columnKeyOverrides).toEqual({
      startAddress: "start_address",
      endAddress: "end_address",
      vrfName: "vrf",
      markUtilized: "mark_utilized",
      markPopulated: "mark_populated",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.vrf).toMatchObject({ param: "vrf_id", type: "fk", fkEntity: "VRFs" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(byKey.mark_utilized).toMatchObject({ param: "mark_utilized", type: "enum" });
    expect(byKey.mark_populated).toMatchObject({ param: "mark_populated", type: "enum" });
  });

  it("matches the module column metadata and ModuleListQuery filter params", () => {
    const config = inventoryTableConfigFor("Modules");
    expect(config.sortKeys).toEqual([
      "device_id",
      "module_bay_id",
      "module_type_id",
      "status",
      "serial",
      "asset_tag",
      "description",
    ]);
    expect(config.identity).toEqual({
      key: "module_type_id",
      label: "inventory.resources.fields.moduleType",
      fieldKey: "moduleTypeName",
    });
    expect(config.columnKeyOverrides).toEqual({
      deviceName: "device_id",
      moduleTypeName: "module_type_id",
      moduleBayName: "module_bay_id",
      serialNumber: "serial",
      assetTagModule: "asset_tag",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.device_id).toMatchObject({ param: "device_id", type: "fk", fkEntity: "Devices" });
    expect(byKey.module_type_id).toMatchObject({ param: "module_type_id", type: "fk", fkEntity: "Module Types" });
    expect(config.filterColumns.some((c) => c.param === "module_bay_id")).toBe(false);
  });

  it("matches the rack reservation column metadata and RackReservationListQuery filter params", () => {
    const config = inventoryTableConfigFor("Rack Reservations");
    expect(config.sortKeys).toEqual(["units", "rack_id", "user_id", "status", "tenant_id", "description", "comments"]);
    expect(config.identity).toEqual({
      key: "rack_id",
      label: "inventory.resources.fields.rack",
      fieldKey: "rackName",
    });
    expect(config.columnKeyOverrides).toEqual({ rackName: "rack_id", userName: "user_id", tenant: "tenant_id" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.rack_id).toMatchObject({ param: "rack_id", type: "fk", fkEntity: "Racks" });
    expect(byKey.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(config.filterColumns.some((c) => c.param === "user_id")).toBe(false);
  });

  it("matches rack type column metadata and RackTypeListQuery filter params", () => {
    const config = inventoryTableConfigFor("Rack Types");
    expect(config.sortKeys).toEqual([
      "external_id",
      "model",
      "manufacturer",
      "slug",
      "form_factor",
      "width",
      "u_height",
      "starting_unit",
      "desc_units",
      "outer_width",
      "outer_depth",
      "outer_height",
      "outer_unit",
      "mounting_depth",
      "weight",
      "max_weight",
      "weight_unit",
      "description",
      "tags",
      "piping_synced_at",
    ]);
    expect(config.identity).toEqual({ key: "model", label: "Model" });
    expect(config.columnKeyOverrides).toMatchObject({ pipingSyncedAt: "piping_synced_at" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.manufacturer).toMatchObject({ param: "manufacturer_id", type: "fk", fkEntity: "Manufacturers" });
    expect(byKey.form_factor).toMatchObject({ param: "form_factor", type: "enum" });
    expect(byKey.desc_units).toMatchObject({ param: "desc_units", type: "enum" });
    expect(byKey.outer_unit).toMatchObject({ param: "outer_unit", type: "enum" });
    expect(byKey.weight_unit).toMatchObject({ param: "weight_unit", type: "enum" });
    expect((byKey.form_factor as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toContain("4-post-cabinet");
    expect((byKey.outer_unit as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["mm", "in"]);
    expect((byKey.weight_unit as { enumValues: { value: string }[] }).enumValues.map((v) => v.value)).toEqual(["kg", "g", "lb", "oz"]);
  });

  it("matches the mac_address column metadata and representable filter params", () => {
    const config = inventoryTableConfigFor("MAC Addresses");
    expect(config.sortKeys).toEqual(["mac_address", "assigned_object_type_id", "assigned_object_id", "description", "comments"]);
    expect(config.identity).toEqual({
      key: "mac_address",
      label: "inventory.resources.fields.macAddress",
      fieldKey: "macAddress",
    });
    expect(config.columnKeyOverrides).toEqual({ macAddress: "mac_address", assignedObject: "assigned_object_id" });

    const byKey = byColKey(config.filterColumns);
    expect(byKey.assigned_object_id).toMatchObject({
      param: "assigned_object_id",
      type: "fk",
      fkEntity: ["Interfaces", "VM Interfaces"],
    });
    expect(byKey.assigned_object_type_id).toBeUndefined();
  });

  it("matches location column metadata and LocationListQuery filter params", () => {
    const config = inventoryTableConfigFor("Locations");
    expect(config.sortKeys).toEqual([
      "external_id",
      "name",
      "slug",
      "site",
      "parent",
      "status",
      "tenant",
      "facility",
      "description",
      "tags",
      "piping_synced_at",
    ]);
    expect(config.columnKeyOverrides).toEqual({ parentName: "parent", pipingSyncedAt: "piping_synced_at" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.site).toMatchObject({ param: "site_id", type: "fk", fkEntity: "Sites" });
    expect(byKey.parent).toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Locations" });
    expect(byKey.tenant).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the virtual_device_context column metadata and filter params", () => {
    const config = inventoryTableConfigFor("Virtual Device Contexts");
    expect(config.sortKeys).toEqual([
      "name",
      "status",
      "identifier",
      "device_id",
      "primary_ip4_id",
      "primary_ip6_id",
      "tenant_id",
      "description",
      "comments",
    ]);
    expect(config.columnKeyOverrides).toEqual({
      deviceName: "device_id",
      primaryIp4: "primary_ip4_id",
      primaryIp6: "primary_ip6_id",
      tenant: "tenant_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.device_id).toMatchObject({ param: "device_id", type: "fk", fkEntity: "Devices" });
    expect(byKey.primary_ip4_id).toMatchObject({ param: "primary_ip4_id", type: "fk", fkEntity: "IP Addresses" });
    expect(byKey.primary_ip6_id).toMatchObject({ param: "primary_ip6_id", type: "fk", fkEntity: "IP Addresses" });
    expect(byKey.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
  });

  it("matches the service_template column metadata and filter params", () => {
    const config = inventoryTableConfigFor("Service Templates");
    expect(config.sortKeys).toEqual(["name", "protocol", "ports", "description", "comments"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.protocol).toMatchObject({ param: "protocol", type: "enum" });
    expect((byKey.protocol as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["tcp", "udp", "sctp"]);
  });

  it("matches the virtual_disk column metadata and safely representable filters", () => {
    const config = inventoryTableConfigFor("Virtual Disks");
    expect(config.sortKeys).toEqual(["virtual_machine_id", "name", "size", "description", "owner_id"]);
    expect(config.columnKeyOverrides).toEqual({ virtualMachineName: "virtual_machine_id", ownerName: "owner_id" });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.virtual_machine_id).toMatchObject({
      param: "virtual_machine_id",
      type: "fk",
      fkEntity: "Virtual Machines",
    });
    // owner_id targets auth users, which are not an InventoryEntityKey source here.
    expect(byKey.owner_id).toBeUndefined();
    expect(config.filterColumns.some((c) => c.param === "owner_id")).toBe(false);
  });

  it("matches module type and virtual chassis filter metadata", () => {
    const moduleTypes = inventoryTableConfigFor("Module Types");
    expect(moduleTypes.sortKeys).toEqual([
      "model",
      "manufacturer",
      "part_number",
      "weight",
      "weight_unit",
      "airflow",
      "description",
      "comments",
    ]);
    expect(moduleTypes.identity).toEqual({ key: "model", label: "Model" });
    expect(moduleTypes.columnKeyOverrides).toEqual({ manufacturerName: "manufacturer" });
    expect(byColKey(moduleTypes.filterColumns).manufacturer)
      .toMatchObject({ param: "manufacturer_id", type: "fk", fkEntity: "Manufacturers" });

    const virtualChassis = inventoryTableConfigFor("Virtual Chassis");
    expect(virtualChassis.sortKeys).toEqual([
      "name",
      "domain",
      "master_id",
      "description",
      "comments",
      "member_count",
      "owner_id",
    ]);
    expect(virtualChassis.columnKeyOverrides).toEqual({ masterName: "master_id", ownerName: "owner_id" });
    const vcFilters = byColKey(virtualChassis.filterColumns);
    expect(vcFilters.master_id).toMatchObject({ param: "master_id", type: "fk", fkEntity: "Devices" });
    expect(vcFilters.owner_id).toBeUndefined();
  });

  it("matches ASN range and rack role list metadata", () => {
    const asnRanges = inventoryTableConfigFor("ASN Ranges");
    expect(asnRanges.sortKeys).toEqual([
      "name",
      "slug",
      "rir_id",
      "start",
      "end",
      "tenant_id",
      "description",
      "comments",
      "owner_id",
    ]);
    const asnRangeFilters = byColKey(asnRanges.filterColumns);
    expect(asnRanges.columnKeyOverrides).toMatchObject({ ownerName: "owner_id" });
    expect(asnRangeFilters.rir_id).toMatchObject({ param: "rir_id", type: "fk", fkEntity: "RIRs" });
    expect(asnRangeFilters.tenant_id).toMatchObject({ param: "tenant_id", type: "fk", fkEntity: "Tenants" });
    expect(asnRangeFilters.owner_id).toBeUndefined();

    const rackRoles = inventoryTableConfigFor("Rack Roles");
    expect(rackRoles.sortKeys).toEqual(["external_id", "name", "slug", "color", "description", "tags", "piping_synced_at"]);
    expect(rackRoles.columnKeyOverrides).toEqual({ pipingSyncedAt: "piping_synced_at" });
    expect(rackRoles.filterColumns).toEqual([]);
  });

  it("matches the inventory_items column metadata and accepted filter params", () => {
    const config = inventoryTableConfigFor("Inventory Items");
    expect(config.sortKeys).toEqual([
      "name",
      "label",
      "part_id",
      "serial",
      "asset_tag",
      "discovered",
      "status",
      "device_id",
      "parent_id",
      "role_id",
      "manufacturer_id",
    ]);
    expect(config.columnKeyOverrides).toEqual({
      deviceName: "device_id",
      parentName: "parent_id",
      role: "role_id",
      manufacturerName: "manufacturer_id",
      partId: "part_id",
    });
    const byKey = byColKey(config.filterColumns);
    expect(byKey.status).toMatchObject({ param: "status", type: "enum" });
    expect(byKey.device_id).toMatchObject({ param: "device_id", type: "fk", fkEntity: "Devices" });
    expect(byKey.parent_id).toMatchObject({ param: "parent_id", type: "fk", fkEntity: "Inventory Items" });
    expect(byKey.role_id).toMatchObject({ param: "role_id", type: "fk", fkEntity: "Inventory Item Roles" });
    expect(byKey.manufacturer_id).toMatchObject({ param: "manufacturer_id", type: "fk", fkEntity: "Manufacturers" });
    expect(byKey.discovered).toMatchObject({ param: "discovered", type: "enum" });
  });

  it("matches the service SERVICE_COLUMNS allowlist and safely representable filters", () => {
    const config = inventoryTableConfigFor("Services");
    expect(config.sortKeys).toEqual(["name", "protocol", "ports", "parentObject", "description"]);
    const byKey = byColKey(config.filterColumns);
    expect(byKey.protocol).toMatchObject({ param: "protocol", type: "enum" });
    expect((byKey.protocol as { enumValues: { value: string }[] }).enumValues.map((v) => v.value))
      .toEqual(["tcp", "udp", "sctp"]);
    expect(byKey.parentObject).toMatchObject({
      param: "parent_object_id",
      type: "fk",
      fkEntity: ["Devices", "Virtual Machines"],
    });

    // owner_id targets auth users, which are not an InventoryEntityKey source here.
    expect(byKey.owner).toBeUndefined();
    expect(config.filterColumns.some((c) => c.param === "owner_id")).toBe(false);
  });
});
