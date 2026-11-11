import { useState } from "react";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate, validateEntityDraft } from "@/shared/forms/entityForm";
import { descriptorFromSpec } from "@/shared/nested-editor/descriptorFromSpec";
import { relationshipsByTab } from "./components/inventory/inventoryTypes";

beforeEach(async () => { await i18n.changeLanguage("en"); });
const t = (key: string) => key;

describe("Module bay templates forms", () => {
  it("requires a name and exactly one hardware parent and defaults enabled on new rows", () => {
    const draft = buildEntityDraft("Module Bay Templates");
    expect(draft.enabled).toBe("true");
    expect(validateEntityDraft("Module Bay Templates", draft, t)).toHaveProperty("name");
    expect(validateEntityDraft("Module Bay Templates", { ...draft, name: "Slot" }, t)).toHaveProperty("deviceTypeId");
    expect(validateEntityDraft("Module Bay Templates", { ...draft, name: "Slot", deviceTypeId: "dt", moduleTypeId: "mt" }, t)).toHaveProperty("deviceTypeId");
    expect(validateEntityDraft("Module Bay Templates", { ...draft, name: "Slot", moduleTypeId: "mt" }, t)).toEqual({});
    expect(validateEntityDraft("Module Bay Templates", { ...draft, name: "x".repeat(65), moduleTypeId: "mt" }, t)).toHaveProperty("name");
  });

  it.each(["Device Types", "Module Types"] as const)("inherits the %s parent in the shared nested editor", parent => {
    const spec = relationshipsByTab[parent]!.find(spec => spec.childEntity === "Module Bay Templates")!;
    const desc = descriptorFromSpec(spec, t);
    const draft = { ...desc.buildDraft(), name: "Slot" };
    expect(desc.validate(draft)).toEqual({});
    expect(desc.toPayload(draft, "hardware")).toMatchObject({ [spec.parentFkField]: "hardware", name: "Slot", enabled: "true", moduleBayTypes: [] });
  });

  it("uses the shared UUID membership picker, clears the last category and renders no phantom metadata", () => {
    function Form() {
      const resource = { id: "slot", name: "Slot", type: "module_bay_template", moduleTypeId: "mt", moduleBayTypes: ["category"], enabled: "false" };
      const initial = buildEntityDraft("Module Bay Templates", resource);
      const [draft, setDraft] = useState(initial);
      return <><EntityFormBody entity="Module Bay Templates" draft={draft} onField={(key, value) => setDraft(current => ({ ...current, [key]: value }))}
        loadOptions={entity => entity === "Module Bay Types" ? [{ id: "category", name: "SFP" }] : []} onRequestCreate={() => undefined} />
        <output data-testid="template-payload">{JSON.stringify(draftToResourceUpdate("Module Bay Templates", draft, "Module Bay Template", resource, initial))}</output></>;
    }
    render(<AllPermissions><Form /></AllPermissions>);
    expect(screen.getByRole("combobox", { name: "Enabled" })).toHaveTextContent("false");
    expect(screen.queryByRole("combobox", { name: "Owner" })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Tags" })).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Comments" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove SFP" }));
    expect(JSON.parse(screen.getByTestId("template-payload").textContent)).toEqual({ id: "slot", moduleBayTypes: [] });
  });
});
