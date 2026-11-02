import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate } from "@/shared/forms/entityForm";
import { ContentTypeScopeField } from "./components/inventory/ContentTypeScopeField";
import { inventoryResourceOption, tagAllowsAttachment } from "./resourceOptions";
import type { ContentType } from "./custom-fields-api";

const tagId = "00000000-0000-4000-8000-000000009401";
const contentTypes: [ContentType, ContentType, ContentType] = [
  { id: "00000000-0000-4000-8000-000000000001", app_label: "dcim", model: "device", token: "dcim.device", label: "Device", supported_for_custom_fields: true, capabilities: { tags: true, custom_fields: true, change_dates: true } },
  { id: "00000000-0000-4000-8000-000000000002", app_label: "dcim", model: "site", token: "dcim.site", label: "Site", supported_for_custom_fields: true, capabilities: { tags: true, custom_fields: true, change_dates: true } },
  { id: "00000000-0000-4000-8000-000000000003", app_label: "dcim", model: "modulebaytemplate", token: "dcim.modulebaytemplate", label: "Module bay template", supported_for_custom_fields: false, capabilities: { tags: false, custom_fields: false, change_dates: false } },
];

function TagScopeForm() {
  const resource = { id: tagId, name: "Core", type: "tag", slug: "core", color: "155dfc", weight: "0", objectTypes: ["dcim.device"] };
  const initial = buildEntityDraft("Tags", resource);
  const [draft, setDraft] = useState(initial);
  return <><EntityFormBody entity="Tags" draft={draft} loadOptions={() => []} onRequestCreate={() => undefined}
    onField={(key, value) => setDraft(previous => ({ ...previous, [key]: value }))} />
    <output data-testid="scope-payload">{JSON.stringify(draftToResourceUpdate("Tags", draft, "Tag", resource, initial))}</output></>;
}

beforeEach(async () => { await i18n.changeLanguage("en"); });

describe("native Tag scope controls", () => {
  it("uses the shared registry picker, preserves omission and clears explicitly to []", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    client.setQueryData(["inventory-content-types"], contentTypes);
    render(<QueryClientProvider client={client}><AllPermissions><TagScopeForm /></AllPermissions></QueryClientProvider>);
    expect(JSON.parse(screen.getByTestId("scope-payload").textContent)).not.toHaveProperty("objectTypes");
    fireEvent.click(screen.getByRole("button", { name: "Remove Device · dcim.device" }));
    expect(JSON.parse(screen.getByTestId("scope-payload").textContent)).toHaveProperty("objectTypes", []);
    fireEvent.click(screen.getByRole("combobox", { name: "Object types" }));
    expect(screen.queryByRole("option", { name: /Module bay template/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "Site · dcim.site" }));
    expect(JSON.parse(screen.getByTestId("scope-payload").textContent)).toHaveProperty("objectTypes", ["dcim.site"]);
  });

  it("shares the same control with CFs while CF values remain content-type IDs", () => {
    const onValueChange = vi.fn();
    render(<ContentTypeScopeField contentTypes={contentTypes} value={[]} onValueChange={onValueChange}
      label="Applies to" valueKind="id" capability="custom-fields" />);
    fireEvent.click(screen.getByRole("combobox", { name: "Applies to" }));
    fireEvent.click(screen.getByRole("option", { name: "Device · dcim.device" }));
    expect(onValueChange).toHaveBeenCalledWith([contentTypes[0].id]);
  });

  it("offers only compatible and explicitly unrestricted Tags without dropping saved selections", () => {
    const onField = vi.fn();
    const options = [
      { id: "any", name: "Any target", objectTypes: [] },
      { id: "group", name: "Wireless only", objectTypes: ["wireless.wirelesslangroup"] },
      { id: "device", name: "Device only", objectTypes: ["dcim.device"] },
      { id: "unknown", name: "Missing metadata" },
    ];
    render(<AllPermissions><EntityFormBody entity="Wireless LAN Groups" draft={{ tags: "device" }}
      visibleKeys={new Set(["tags"])} onField={onField} loadOptions={() => options} onRequestCreate={() => undefined} /></AllPermissions>);
    expect(screen.getByRole("button", { name: "Remove device" })).toBeInTheDocument();
    expect(onField).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("combobox", { name: "Tags" }));
    expect(screen.getByRole("option", { name: "Any target" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Wireless only" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Device only" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Missing metadata" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "Wireless only" }));
    expect(onField).toHaveBeenCalledWith("tags", "device,group");
  });

  it("keeps scoped metadata on shared catalogue options and fails closed for unknown attachment types", () => {
    const option = inventoryResourceOption("Tags", { id: tagId, name: "Core", type: "tag", objectTypes: ["dcim.device"] });
    expect(option.objectTypes).toEqual(["dcim.device"]);
    expect(tagAllowsAttachment(option, "dcim.device")).toBe(true);
    expect(tagAllowsAttachment(option, "dcim.site")).toBe(false);
    expect(tagAllowsAttachment(option, null)).toBe(false);
    expect(tagAllowsAttachment({ objectTypes: [] }, null)).toBe(true);
    expect(tagAllowsAttachment({}, "dcim.device")).toBe(false);
  });
});
