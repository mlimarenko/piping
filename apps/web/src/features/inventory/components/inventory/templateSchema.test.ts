import { describe, expect, it } from "vitest";

import type { InventoryTemplate } from "@/shared/api/types";

import {
  TEMPLATE_RESOURCE_DEFINITIONS,
  autoBindStepReferences,
  buildTemplatePreview,
  buildTemplateVariableValues,
  clearStepReferences,
  renameStepReferences,
  templateReferenceFor,
  validateTemplate,
} from "./templateSchema";
import type { TemplateStep, TemplateVariable } from "@/shared/api/types";

function baseTemplate(overrides: Partial<InventoryTemplate> = {}): InventoryTemplate {
  return {
    id: "",
    name: "Provision site",
    slug: "provision-site",
    status: "draft",
    description: "",
    updatedAt: "",
    variables: [],
    steps: [
      {
        step_key: "site",
        title: "Create site",
        resource_type: "site",
        operation: "create",
        depends_on_step_keys: [],
        payload_template: { name: "{{site_name}}" },
        on_conflict_policy: "fail",
      },
    ],
    ...overrides,
  };
}

describe("templateSchema", () => {
  it("keeps JSON-looking manual string defaults literal even for name-reference fields", () => {
    const variable: TemplateVariable = { key: "role", label: "Role", value_type: "string", default_value: "{manual role}" };
    const template = baseTemplate({ variables: [variable], steps: [{ step_key: "device", title: "Device", resource_type: "device", operation: "create", depends_on_step_keys: [], on_conflict_policy: "fail", payload_template: { name: "Switch", site_id: "site-id", role: "{{role}}", device_type_id: "type-id" } }] });
    const result = validateTemplate(template);
    expect(result.issues.filter((issue) => issue.section === "variables")).toEqual([]);
    expect(result.normalized.variables[0]?.default_value).toBe("{manual role}");
  });

  it("preserves typed reference inputs and ordinary JSON objects independently", () => {
    const selected = { resource: "device_type", id: "00000000-0000-4000-8000-000000000001" };
    const result = buildTemplateVariableValues([
      { key: "chosen", label: "Type", value_type: "string" },
      { key: "data", label: "Data", value_type: "json" },
    ], { chosen: selected, data: JSON.stringify({ ...selected, extra: "ordinary data" }) });
    expect(result.errors).toEqual({});
    expect(result.values).toEqual({ chosen: selected, data: { ...selected, extra: "ordinary data" } });
  });

  it("accepts same-target name/ID consumers but keeps interpolated creation text editable", () => {
    const variable: TemplateVariable = { key: "chosen", label: "Choice", value_type: "string" };
    const step: TemplateStep = { step_key: "device", title: "Device", resource_type: "device", operation: "create", depends_on_step_keys: [], on_conflict_policy: "fail", payload_template: { role: "{{chosen}}", role_id: "{{ chosen }}" } };
    const template = baseTemplate({ variables: [variable], steps: [step] });
    expect(templateReferenceFor(template, variable)).toEqual({ resource: "device_role", entity: "Device Roles", value: "id" });
    step.payload_template.name = "Device {{chosen}}";
    expect(templateReferenceFor(template, variable)).toBeNull();
    delete step.payload_template.name;
    step.payload_template.role = "{{cidr_host chosen 1}}";
    expect(templateReferenceFor(template, variable)).toBeNull();
  });

  it("keeps mixed creation/reference consumers free text and supports contact-role references", () => {
    const variable: TemplateVariable = { key: "site_name", value_type: "string", label: "Site" };
    const template = baseTemplate({ variables: [variable] });
    const site: TemplateStep = { step_key: "site", title: "Site", resource_type: "site", operation: "create", depends_on_step_keys: [], payload_template: { name: "{{site_name}}" }, on_conflict_policy: "fail" };
    template.steps.push({ ...site, step_key: "prefix", resource_type: "prefix", payload_template: { site: "{{site_name}}" } });
    expect(templateReferenceFor(template, variable)).toBeNull();
    template.steps.reverse();
    expect(templateReferenceFor(template, variable)).toBeNull();
    expect(templateReferenceFor(template, { ...variable, value_type: "resource_ref", validation_rules: { resource: "site" } })).toBeNull();
    template.variables = [{ ...variable, value_type: "resource_ref", validation_rules: { resource: "site", options: ["New site"] } }];
    expect(validateTemplate(template).normalized.variables[0]?.validation_rules).toEqual({ options: ["New site"] });
    const role: TemplateVariable = { key: "role", value_type: "resource_ref", label: "Contact role", validation_rules: { resource: "contact_role" } };
    expect(templateReferenceFor(baseTemplate(), role)).toEqual({ resource: "contact_role", entity: "Contact Roles", value: "id" });
    const assignment = baseTemplate({ variables: [role], steps: [{ ...site, resource_type: "contact_assignment", payload_template: { role_id: "{{role}}" } }] });
    expect(templateReferenceFor(assignment, role)?.entity).toBe("Contact Roles");
    const emptyAssignment = baseTemplate({ steps: [{ ...site, resource_type: "contact_assignment", payload_template: {} }] });
    expect(autoBindStepReferences(emptyAssignment, 0).variables.find((item) => item.key === "role_id")?.validation_rules).toEqual({ resource: "contact_role" });
  });

  it("coerces run variables to canonical JSON values", () => {
    const { values, errors } = buildTemplateVariableValues(
      [
        { key: "site_name", label: "Site name", value_type: "string", required: true },
        { key: "latitude", label: "Latitude", value_type: "number" },
        { key: "enabled", label: "Enabled", value_type: "boolean" },
        { key: "metadata", label: "Metadata", value_type: "json" },
      ],
      {
        site_name: "edge-1",
        latitude: "55.751244",
        enabled: "true",
        metadata: '{"tier":"edge"}',
      },
    );

    expect(errors).toEqual({});
    expect(values).toEqual({
      site_name: "edge-1",
      latitude: 55.751244,
      enabled: true,
      metadata: { tier: "edge" },
    });
  });

  it("builds preview payloads from canonical step schema", () => {
    const template: InventoryTemplate = {
      id: "template-1",
      name: "Provision site",
      slug: "provision-site",
      status: "draft",
      description: "",
      updatedAt: "",
      variables: [
        { key: "site_name", label: "Site name", value_type: "string", required: true },
        { key: "latitude", label: "Latitude", value_type: "number" },
        { key: "enabled", label: "Enabled", value_type: "boolean" },
      ],
      steps: [
        {
          step_key: "site",
          title: "Create site",
          resource_type: "site",
          operation: "create",
          depends_on_step_keys: [],
          payload_template: {
            name: "{{site_name}}",
            latitude: "{{latitude}}",
            description: "site {{site_name}}",
            comments: "{{steps.tenant.id}}",
          },
          on_conflict_policy: "fail",
        },
        {
          step_key: "device_interface",
          title: "Create interface",
          resource_type: "device_interface",
          operation: "create",
          depends_on_step_keys: ["site"],
          payload_template: {
            name: "mgmt0",
            mgmt_only: "{{enabled}}",
          },
          on_conflict_policy: "fail",
        },
      ],
    };

    const preview = buildTemplatePreview(template, {
      site_name: "edge-1",
      latitude: 55.751244,
      enabled: true,
    });

    expect(preview).toHaveLength(2);
    expect(preview[0]).toEqual({
      step_key: "site",
      title: "Create site",
      resource_type: "site",
      payload: {
        name: "edge-1",
        latitude: 55.751244,
        description: "site edge-1",
        comments: "{{steps.tenant.id}}",
      },
    });
    expect(preview[1]?.payload).toEqual({
      name: "mgmt0",
      mgmt_only: true,
    });
  });
});

describe("validateTemplate", () => {
  it("accepts a well-formed template and normalizes it", () => {
    const result = validateTemplate(baseTemplate());
    expect(result.issues).toEqual([]);
    expect(result.normalized.steps[0]?.sort_order).toBe(0);
    expect(result.normalized.name).toBe("Provision site");
  });

  it("flags a missing name and a malformed slug, addressable by target", () => {
    const result = validateTemplate(baseTemplate({ name: "  ", slug: "Bad Slug!" }));
    const targets = result.issues.map((i) => i.target);
    expect(targets).toContain("meta-name");
    expect(targets).toContain("meta-slug");
    expect(result.issues.find((i) => i.target === "meta-name")?.code).toBe("nameRequired");
    expect(result.issues.find((i) => i.target === "meta-slug")?.code).toBe("slugFormat");
  });

  it("requires at least one step", () => {
    const result = validateTemplate(baseTemplate({ steps: [] }));
    expect(result.issues.some((i) => i.code === "noSteps")).toBe(true);
  });

  it("detects duplicate variable keys and unknown dependencies", () => {
    const result = validateTemplate(
      baseTemplate({
        variables: [
          { key: "dup", label: "A", value_type: "string", required: false },
          { key: "dup", label: "B", value_type: "string", required: false },
        ],
        steps: [
          {
            step_key: "site",
            title: "Create site",
            resource_type: "site",
            operation: "create",
            depends_on_step_keys: ["ghost"],
            payload_template: { name: "x" },
            on_conflict_policy: "fail",
          },
        ],
      }),
    );
    expect(result.issues.some((i) => i.code === "duplicateVariableKey")).toBe(true);
    expect(result.issues.some((i) => i.code === "unknownDependency")).toBe(true);
  });

  it("flags a required payload field left blank", () => {
    const result = validateTemplate(
      baseTemplate({
        steps: [
          {
            step_key: "site",
            title: "Create site",
            resource_type: "site",
            operation: "create",
            depends_on_step_keys: [],
            payload_template: {}, // `name` is required for `site`
            on_conflict_policy: "fail",
          },
        ],
      }),
    );
    const issue = result.issues.find((i) => i.code === "requiredStepField");
    expect(issue).toBeDefined();
    expect(issue?.target).toBe("step-0-field-name");
  });
});

describe("autoBindStepReferences", () => {
  const siteStep: TemplateStep = {
    step_key: "create_site",
    title: "Create site",
    resource_type: "site",
    operation: "create",
    depends_on_step_keys: [],
    payload_template: { name: "{{site_name}}" },
    on_conflict_policy: "fail",
  };
  const deviceStep: TemplateStep = {
    step_key: "create_device",
    title: "Create device",
    resource_type: "device",
    operation: "create",
    depends_on_step_keys: [],
    payload_template: { name: "{{device_name}}" },
    on_conflict_policy: "fail",
  };

  it("links a required FK to the nearest prior step of the referenced type", () => {
    const result = autoBindStepReferences(
      baseTemplate({ steps: [siteStep, deviceStep] }),
      1,
    );

    const device = result.steps[1]!;
    // site_id resolves to the real prior step key, not the stale `{{steps.site.id}}` hint.
    expect(device.payload_template.site_id).toBe("{{steps.create_site.id}}");
    expect(device.depends_on_step_keys).toContain("create_site");
    // No prior step for role_id / device_type_id → generated run variables.
    expect(device.payload_template.role_id).toBe("{{role_id}}");
    expect(device.payload_template.device_type_id).toBe("{{device_type_id}}");
    const varKeys = result.variables.map((v) => v.key);
    expect(varKeys).toEqual(expect.arrayContaining(["role_id", "device_type_id"]));
    const roleVar = result.variables.find((v) => v.key === "role_id")!;
    expect(roleVar.value_type).toBe("resource_ref");
    expect(roleVar.required).toBe(true);
    expect(roleVar.label).toBe("Role");
  });

  it("falls back to a generated variable when no prior step matches", () => {
    const result = autoBindStepReferences(baseTemplate({ steps: [deviceStep] }), 0);

    const device = result.steps[0]!;
    expect(device.payload_template.site_id).toBe("{{site_id}}");
    expect(device.depends_on_step_keys).toEqual([]);
    expect(result.variables.map((v) => v.key)).toEqual(
      expect.arrayContaining(["site_id", "role_id", "device_type_id"]),
    );
  });

  it("never overwrites a manually set field and never duplicates a variable", () => {
    const result = autoBindStepReferences(
      baseTemplate({
        variables: [
          { key: "role_id", label: "Existing", value_type: "string", required: false },
        ],
        steps: [{ ...deviceStep, payload_template: { name: "x", site_id: "manual-uuid" } }],
      }),
      0,
    );

    const device = result.steps[0]!;
    expect(device.payload_template.site_id).toBe("manual-uuid");
    // The pre-existing role_id variable is kept as-is (no duplicate, no clobber).
    expect(result.variables.filter((v) => v.key === "role_id")).toHaveLength(1);
    expect(result.variables.find((v) => v.key === "role_id")?.label).toBe("Existing");
  });
});

describe("renameStepReferences / clearStepReferences", () => {
  const steps: TemplateStep[] = [
    {
      step_key: "create_site",
      title: "Create site",
      resource_type: "site",
      operation: "create",
      depends_on_step_keys: [],
      payload_template: { name: "x" },
      on_conflict_policy: "fail",
    },
    {
      step_key: "create_device",
      title: "Create device",
      resource_type: "device",
      operation: "create",
      depends_on_step_keys: ["create_site"],
      payload_template: { name: "y", site_id: "{{steps.create_site.id}}" },
      on_conflict_policy: "fail",
    },
  ];

  it("cascades a step-key rename into dependent payloads and deps", () => {
    const renamed = renameStepReferences(steps, "create_site", "primary_site");
    expect(renamed[1]?.payload_template.site_id).toBe("{{steps.primary_site.id}}");
    expect(renamed[1]?.depends_on_step_keys).toEqual(["primary_site"]);
  });

  it("clears references to a removed step so required validation re-triggers", () => {
    const cleared = clearStepReferences(steps, "create_site");
    expect(cleared[1]?.payload_template.site_id).toBe("");
  });
});

// keep in sync with backend SUPPORTED_RESOURCE_TYPES in template_step_resource_repository.rs
describe("TEMPLATE_RESOURCE_DEFINITIONS drift guard", () => {
  it("covers exactly the backend-supported resource types", () => {
    const EXPECTED_RESOURCE_TYPES = new Set([
      "tenant",
      "site",
      "device",
      "virtual_machine",
      "device_interface",
      "vm_interface",
      "ip_address",
      "tenant_group",
      "region",
      "site_group",
      "location",
      "prefix",
      "rack",
      "rack_role",
      "rack_type",
      "manufacturer",
      "device_role",
      "device_type",
      "contact",
      "contact_assignment",
      "platform",
      "cluster",
      "cluster_type",
      "cluster_group",
    ]);

    const actual = new Set(TEMPLATE_RESOURCE_DEFINITIONS.map((d) => d.resourceType));
    expect(actual).toEqual(EXPECTED_RESOURCE_TYPES);
  });
});

describe("template reference variables", () => {
  const siteStep: TemplateStep = {
    step_key: "create_site",
    title: "Create site",
    resource_type: "site",
    operation: "create",
    depends_on_step_keys: [],
    payload_template: { name: "{{site_name}}" },
    on_conflict_policy: "fail",
  };
  // The factory shape: the step consumes role/device_type BY NAME.
  const nameBoundDeviceStep: TemplateStep = {
    step_key: "create_device",
    title: "Create device",
    resource_type: "device",
    operation: "create",
    depends_on_step_keys: ["create_site"],
    payload_template: {
      name: "{{device_name}}",
      site_id: "{{steps.create_site.id}}",
      role: "{{device_role}}",
      device_type: "{{device_type}}",
    },
    on_conflict_policy: "fail",
  };
  // The canonical authoring shape: the step consumes the object UUIDs.
  const idBoundDeviceStep: TemplateStep = {
    ...nameBoundDeviceStep,
    payload_template: {
      name: "{{device_name}}",
      site_id: "{{steps.create_site.id}}",
      role_id: "{{device_role}}",
      device_type_id: "{{device_type}}",
    },
  };
  const stringVariable = (key: string): TemplateVariable => ({
    key,
    label: key,
    value_type: "string",
    required: true,
  });
  const referenceVariable = (rules?: Record<string, unknown>): TemplateVariable => ({
    key: "role_id",
    label: "Role",
    value_type: "resource_ref",
    required: true,
    ...(rules ? { validation_rules: rules } : {}),
  });

  it("declares the referenced resource on generated reference variables", () => {
    const unboundDevice: TemplateStep = {
      ...idBoundDeviceStep,
      depends_on_step_keys: [],
      payload_template: { name: "{{device_name}}" },
    };
    const result = autoBindStepReferences(baseTemplate({ steps: [unboundDevice] }), 0);

    expect(result.variables.find((v) => v.key === "role_id")?.validation_rules).toEqual({
      resource: "device_role",
    });
    expect(result.variables.find((v) => v.key === "device_type_id")?.validation_rules).toEqual({
      resource: "device_type",
    });
  });

  it("resolves a name-reference from the step binding and keeps typing as a name", () => {
    const template = baseTemplate({
      variables: [stringVariable("device_role"), stringVariable("device_type")],
      steps: [siteStep, nameBoundDeviceStep],
    });

    expect(templateReferenceFor(template, template.variables[0]!)).toEqual({
      resource: "device_role",
      entity: "Device Roles",
      value: "name",
    });
    expect(templateReferenceFor(template, template.variables[1]!)).toEqual({
      resource: "device_type",
      entity: "Device Types",
      value: "name",
    });
  });

  it("resolves a UUID reference from an *_id binding and from a declared resource", () => {
    const bound = baseTemplate({
      variables: [stringVariable("device_role")],
      steps: [siteStep, idBoundDeviceStep],
    });
    expect(templateReferenceFor(bound, bound.variables[0]!)).toEqual({
      resource: "device_role",
      entity: "Device Roles",
      value: "id",
    });

    const declared = baseTemplate({
      variables: [referenceVariable({ resource: "device_role" })],
      steps: [siteStep],
    });
    expect(templateReferenceFor(declared, declared.variables[0]!)).toEqual({
      resource: "device_role",
      entity: "Device Roles",
      value: "id",
    });
  });

  it("leaves values that are not references unpickable", () => {
    const creation = baseTemplate({
      variables: [stringVariable("site_name"), stringVariable("device_name")],
      steps: [siteStep, nameBoundDeviceStep],
    });
    expect(templateReferenceFor(creation, creation.variables[0]!)).toBeNull();
    expect(templateReferenceFor(creation, creation.variables[1]!)).toBeNull();

    // A declared reference with no consuming reference field has no kind.
    const unbound = baseTemplate({ variables: [referenceVariable()], steps: [siteStep] });
    expect(templateReferenceFor(unbound, unbound.variables[0]!)).toBeNull();
  });

  it("persists the derived resource the next time a legacy template is saved", () => {
    const unboundDevice: TemplateStep = {
      ...idBoundDeviceStep,
      depends_on_step_keys: [],
      payload_template: { name: "{{device_name}}" },
    };
    const bound = autoBindStepReferences(baseTemplate({ steps: [unboundDevice] }), 0);
    // A template authored before the declaration existed keeps the binding but
    // no rule — the next save must persist the derived resource.
    const legacy = baseTemplate({
      steps: bound.steps,
      variables: bound.variables.map((variable) => ({ ...variable, validation_rules: {} })),
    });

    const result = validateTemplate(legacy);

    expect(result.issues).toEqual([]);
    const roleVariable = result.normalized.variables.find((v) => v.key === "role_id");
    expect(roleVariable?.validation_rules).toEqual({ resource: "device_role" });
  });
});
