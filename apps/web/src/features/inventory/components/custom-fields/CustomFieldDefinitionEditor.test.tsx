import type { ReactNode } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { setupServer } from "msw/node";
import i18n from "i18next";
import "@/shared/i18n";
import { handlers } from "@/demo/handlers";
import type { ContentType } from "@/features/inventory/custom-fields-api";
import CustomFieldDefinitionEditor from "./CustomFieldDefinitionEditor";
import { customFieldContentTypeTokenFor } from "@/features/inventory/tableConfig";

const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterAll(() => server.close());

vi.mock("@/shared/ui-shared/EntityEditorHost", () => ({
  default: ({ children, footer }: { children: ReactNode; footer: ReactNode }) => (
    <section>{children}{footer}</section>
  ),
}));

describe("new custom-field scope selection", () => {
  it("selects all three advertised Wireless scopes and submits their content-type IDs", async () => {
    const response = await fetch(`${window.location.origin}/api/v1/inventory/content-types`);
    expect(response.ok).toBe(true);
    const contentTypes: ContentType[] = await response.json();
    const scopes = (["Wireless LAN Groups", "Wireless LANs", "Wireless Links"] as const).map((entity) => {
      const scope = contentTypes.find((type) => type.token === customFieldContentTypeTokenFor(entity));
      if (!scope) throw new Error(`Missing demo content type for ${entity}`);
      return scope;
    });
    const onCreate = vi.fn();
    render(<CustomFieldDefinitionEditor open onOpenChange={vi.fn()} choiceSets={[]}
      contentTypes={contentTypes} contentTypesLoading={false} contentTypesError={false}
      saving={false} onCreate={onCreate} onUpdate={vi.fn()} />);
    fireEvent.change(screen.getByPlaceholderText("cf_owner"), { target: { value: "wireless_notes" } });
    fireEvent.change(screen.getByPlaceholderText(i18n.t("customFields.def.labelPlaceholder")), { target: { value: "Wireless notes" } });
    fireEvent.click(screen.getByRole("combobox", { name: i18n.t("customFields.def.scope") }));
    for (const scope of scopes) {
      fireEvent.click(await screen.findByRole("option", { name: `${scope.label} · ${scope.token}` }));
    }
    fireEvent.click(screen.getByRole("button", { name: i18n.t("create") }));
    expect(onCreate).toHaveBeenCalledWith(expect.objectContaining({
      name: "wireless_notes", object_types: scopes.map((scope) => scope.id),
    }));
  });
  it("selects advertised Rack Group and Cable Bundle scopes and submits their IDs", async () => {
    const response = await fetch(`${window.location.origin}/api/v1/inventory/content-types`);
    expect(response.ok).toBe(true);
    const contentTypes: ContentType[] = await response.json();
    const onCreate = vi.fn();
    render(
      <CustomFieldDefinitionEditor
        open
        onOpenChange={vi.fn()}
        choiceSets={[]}
        contentTypes={contentTypes}
        contentTypesLoading={false}
        contentTypesError={false}
        saving={false}
        onCreate={onCreate}
        onUpdate={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByPlaceholderText("cf_owner"), { target: { value: "installation_notes" } });
    fireEvent.change(screen.getByPlaceholderText(i18n.t("customFields.def.labelPlaceholder")), { target: { value: "Installation notes" } });
    fireEvent.click(screen.getByRole("combobox", { name: i18n.t("customFields.def.scope") }));
    fireEvent.click(await screen.findByRole("option", { name: "Rack Group · dcim.rackgroup" }));
    fireEvent.click(screen.getByRole("option", { name: "Cable Bundle · dcim.cablebundle" }));
    expect(screen.queryByRole("option", { name: "Rack · dcim.rack" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("create") }));
    expect(onCreate).toHaveBeenCalledWith(expect.objectContaining({
      name: "installation_notes",
      object_types: ["ct-rack-group", "ct-cable-bundle"],
    }));
  });
});
