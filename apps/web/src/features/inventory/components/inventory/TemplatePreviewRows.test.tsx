import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "@/shared/i18n";
import TemplatePreviewRows from "./TemplatePreviewRows";
import { templateVariablePresentation, type PreviewRow } from "./templateSchema";
import type { InventoryTemplate } from "@/shared/api/types";
import i18n from "@/shared/i18n";

describe("template launch presentation", () => {
  it("localizes factory defaults by consumer after a variable rename, preserving custom text even on a system template", () => {
    const variable = { key: "renamed", label: "Site name", description: "Site to create for the server.", value_type: "string" as const, required: true };
    const template: InventoryTemplate = { id: "t", name: "T", slug: "t", status: "active", description: "", updatedAt: "", is_system: true, variables: [variable], steps: [{ step_key: "s", title: "S", resource_type: "site", operation: "create", payload_template: { name: "{{renamed}}" }, depends_on_step_keys: [], on_conflict_policy: "fail" }] };
    const translate = (key: string, options?: object) => i18n.t(key, { ...options, lng: "ru" });
    expect(templateVariablePresentation(template, variable, translate)).toMatchObject({ label: "Имя объекта · Площадка", group: "creates", help: "Будет создан новый объект с этим именем." });
    expect(templateVariablePresentation(template, { ...variable, label: "My site", description: "My instructions" }, translate)).toMatchObject({ label: "My site", help: "My instructions" });
    expect(templateVariablePresentation({ ...template, is_system: false }, variable, translate).label).toBe("Site name");
    const mixed = { ...template, steps: [...template.steps, { ...template.steps[0]!, resource_type: "device" as const, payload_template: { role: "{{renamed}}" } }] };
    expect(templateVariablePresentation(mixed, variable, translate)).toMatchObject({ label: "Site name", group: "parameters" });
  });

  it("renders future repeated instances and singleton non-default start, reference context, false/0 and unknown details without UUID replacement", () => {
    const id = "00000000-0000-4000-8000-000000000001";
    const rows: PreviewRow[] = [
      { step_key: "sites[7]", title: "Site", resource_type: "site", payload: { name: "East" } },
      { step_key: "sites[8]", title: "Site", resource_type: "site", payload: { name: "West" } },
      { step_key: "single", title: "Device", resource_type: "device", payload: { name: id, site_id: "preview:sites:8", role_id: id } },
      { step_key: "port", title: "Port", resource_type: "device_interface", payload: { name: "eth0", device_id: "preview:single:9", mgmt_only: false, extra: 0 } },
    ];
    const payload = structuredClone(rows);
    render(<TemplatePreviewRows rows={rows} optionsByEntity={new Map([["Device Roles", { options: [{ id, name: "Server role", subtitle: "Production" }] }]])} />);
    const items = screen.getAllByRole("listitem");
    expect(within(items[2]!).getByText("West · will be created at step 2")).toBeInTheDocument();
    expect(within(items[2]!).getByText("Server role · Production")).toBeInTheDocument();
    expect(within(items[2]!).getByText(id)).toBeInTheDocument();
    expect(within(items[3]!).getByText(`${id} · will be created at step 3`)).toBeInTheDocument();
    expect(within(items[3]!).getByText("No")).toBeInTheDocument();
    expect(within(items[3]!).getByText(/extra/).parentElement).toHaveTextContent("extra: 0");
    expect(rows).toEqual(payload);
  });
});
