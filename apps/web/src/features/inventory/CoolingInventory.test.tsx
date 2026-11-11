import { useState } from "react";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate, validateEntityDraft } from "@/shared/forms/entityForm";
import { descriptorFromSpec } from "@/shared/nested-editor/descriptorFromSpec";
import { relationshipsByTab } from "./components/inventory/inventoryTypes";
import { inventoryResourceOption } from "./resourceOptions";
import { inventoryErrorMessage } from "./inventoryErrors";
import { ApiError } from "@/shared/api/client";

beforeEach(async () => { await i18n.changeLanguage("en"); });
const t = (key: string) => key;

describe("Cooling inventory forms", () => {
  it("shows safe localized graph/reference guidance and deletion conflict feedback without internal errors", () => {
    expect(inventoryErrorMessage(t, new ApiError({ status: 422, message: "private SQL loop detail" }), "Cooling Intakes"))
      .toBe("inventory.resources.saveErrors.coolingValidation");
    expect(inventoryErrorMessage(t, new ApiError({ status: 409, message: "private SQL reference detail" }), "Cooling Sources"))
      .toBe("inventory.resources.saveErrors.conflict");
  });
  it("accepts fractional measurements, rejects invalid precision/ranges and requires unit pairs", () => {
    const draft = { ...buildEntityDraft("Cooling Intakes"), name: "Supply", deviceId: "device", diameter: "12.75", diameterUnit: "mm", maxFlow: "1.25", maxFlowUnit: "m3ph" };
    expect(validateEntityDraft("Cooling Intakes", draft, t)).toEqual({});
    for (const diameter of ["0", "-1", "NaN", "Infinity", "1.001", "1000000"]) {
      expect(validateEntityDraft("Cooling Intakes", { ...draft, diameter }, t)).toHaveProperty("diameter");
    }
    expect(validateEntityDraft("Cooling Intakes", { ...draft, diameterUnit: "" }, t)).toHaveProperty("diameterUnit", "inventory.resources.form.coolingUnitPair");
  });

  it("requires exactly one template parent and injects a nested hardware parent without phantom metadata", () => {
    const draft = { ...buildEntityDraft("Cooling Intake Templates"), name: "Inlet" };
    expect(validateEntityDraft("Cooling Intake Templates", draft, t)).toHaveProperty("deviceTypeId");
    expect(validateEntityDraft("Cooling Intake Templates", { ...draft, deviceTypeId: "d", moduleTypeId: "m" }, t)).toHaveProperty("deviceTypeId");
    for (const parent of ["Device Types", "Module Types"] as const) {
      const spec = relationshipsByTab[parent]!.find(spec => spec.childEntity === "Cooling Intake Templates")!;
      const desc = descriptorFromSpec(spec, t);
      expect(desc.validate(draft)).toEqual({});
      const payload = desc.toPayload(draft, "hardware");
      expect(payload).toHaveProperty(spec.parentFkField, "hardware");
      expect(payload).not.toHaveProperty("ownerId");
      expect(payload).not.toHaveProperty("tags");
    }
  });

  it("preserves explicit clears through nested module/device scopes and leaves untouched measurements omitted", () => {
    const spec = relationshipsByTab.Modules!.find(spec => spec.childEntity === "Cooling Intakes")!;
    const desc = descriptorFromSpec({ ...spec, fixedFields: { deviceId: "device" } }, t);
    const resource = { id: "in", name: "Inlet", type: "cooling_intake", deviceId: "device", moduleId: "module", maxFlow: "1.25", maxFlowUnit: "lpm" };
    const initial = desc.toDraft(resource);
    expect(desc.toPayload({ ...initial, maxFlow: "", maxFlowUnit: "" }, "module", initial)).toEqual({ maxFlow: null, maxFlowUnit: null });
    expect(desc.toPayload({ ...initial, name: "Renamed" }, "module", initial)).toEqual({ name: "Renamed" });
    expect(desc.toPayload({ ...desc.buildDraft(), name: "Fresh", deviceId: "tampered" }, "module")).toMatchObject({ deviceId: "device", moduleId: "module" });
  });

  it("limits outflow intake choices to the selected device and clears them when the device changes", () => {
    const onField = vi.fn();
    render(<AllPermissions><EntityFormBody entity="Cooling Outflows" draft={{ deviceId: "a", coolingIntakeId: "ia" }} onField={onField}
      loadOptions={entity => entity === "Devices" ? [{ id: "a", name: "Device A" }, { id: "b", name: "Device B" }] : entity === "Cooling Intakes" ? [{ id: "ia", name: "Intake A", deviceId: "a" }, { id: "ib", name: "Intake B", deviceId: "b" }] : []}
      onRequestCreate={() => undefined} /></AllPermissions>);
    fireEvent.click(screen.getByRole("combobox", { name: "Cooling intake" }));
    expect(screen.queryByRole("option", { name: /Intake B/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Intake A/ }));
    fireEvent.click(screen.getByRole("combobox", { name: /Device/ }));
    fireEvent.click(screen.getByRole("option", { name: /Device B/ }));
    expect(onField).toHaveBeenCalledWith("coolingIntakeId", "");
  });

  it("constrains feeds to source-site racks and retains the nullable clear action", () => {
    const source = inventoryResourceOption("Cooling Sources", { id: "s", name: "Chiller", type: "cooling_source", siteId: "site-a" });
    const onField = vi.fn();
    render(<AllPermissions><EntityFormBody entity="Cooling Feeds" draft={{ coolingSourceId: "s", rackId: "ra" }} onField={onField}
      loadOptions={entity => entity === "Cooling Sources" ? [source] : entity === "Racks" ? [{ id: "ra", name: "Rack A", siteId: "site-a" }, { id: "rb", name: "Rack B", siteId: "site-b" }] : []}
      onRequestCreate={() => undefined} /></AllPermissions>);
    fireEvent.click(screen.getByRole("combobox", { name: "Rack" }));
    expect(screen.getByRole("option", { name: /Rack A/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Rack B/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Rack A/ }));
    fireEvent.click(screen.getByRole("button", { name: "Clear Rack" }));
    expect(onField).toHaveBeenCalledWith("rackId", "");
  });

  it("constrains template intake choices to the same device or module type", () => {
    render(<AllPermissions><EntityFormBody entity="Cooling Outflow Templates" draft={{ moduleTypeId: "m" }} onField={() => undefined}
      visibleKeys={new Set(["coolingIntakeId"])}
      loadOptions={() => [{ id: "a", name: "Module inlet", moduleTypeId: "m", deviceTypeId: null }, { id: "b", name: "Other inlet", moduleTypeId: "other" }, { id: "c", name: "Device inlet", deviceTypeId: "d" }]}
      onRequestCreate={() => undefined} /></AllPermissions>);
    fireEvent.click(screen.getByRole("combobox", { name: "Cooling intake" }));
    expect(screen.getByRole("option", { name: /Module inlet/ })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Other inlet|Device inlet/ })).not.toBeInTheDocument();
  });

  it("renders decimal input limits and sends an explicit null after clearing", () => {
    function Form() {
      const resource = { id: "s", name: "Chiller", type: "cooling_source", siteId: "site", coolingType: "chiller", coolingCapacity: "12.75" };
      const initial = buildEntityDraft("Cooling Sources", resource);
      const [draft, setDraft] = useState(initial);
      return <><EntityFormBody entity="Cooling Sources" draft={draft} visibleKeys={new Set(["coolingCapacity"])}
        onField={(key, value) => setDraft(current => ({ ...current, [key]: value }))} loadOptions={() => []} onRequestCreate={() => undefined} />
        <output data-testid="cooling-payload">{JSON.stringify(draftToResourceUpdate("Cooling Sources", draft, "Cooling Source", resource, initial))}</output></>;
    }
    render(<AllPermissions><Form /></AllPermissions>);
    const capacity = screen.getByRole("spinbutton", { name: "Cooling capacity (kW)" });
    expect(capacity).toHaveValue(12.75);
    expect(capacity).toHaveAttribute("min", "0");
    expect(capacity).toHaveAttribute("step", "0.01");
    fireEvent.change(capacity, { target: { value: "" } });
    expect(JSON.parse(screen.getByTestId("cooling-payload").textContent)).toEqual({ id: "s", coolingCapacity: null });
  });
});
