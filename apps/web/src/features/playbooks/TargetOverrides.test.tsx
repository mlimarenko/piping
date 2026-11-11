import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { SelectedHost } from "./hostSelection";
import "@/shared/i18n";
import TargetSetSelectorBuilder from "./TargetSetSelectorBuilder";
import { modelToSelector, selectorToModel } from "./selectorModel";

const host: SelectedHost = { id: "11111111-1111-4111-8111-111111111111", name: "Chosen host", subtitle: "192.0.2.1", sourceKind: "device", sourceId: null };
vi.mock("./HostMultiSelectPicker", () => ({ default: ({ onChange }: { onChange: (hosts: SelectedHost[]) => void }) => <>
  <button onClick={() => onChange([host])}>choose host</button>
  <button onClick={() => onChange([])}>clear selection</button>
</> }));

describe("target overrides", () => {
  it.each(["include_target_ids", "exclude_target_ids"] as const)("updates %s by target ID and preserves the other selector operators", async field => {
    const model = selectorToModel({ name: "ordinary facet", [field]: [host.id], exclude_source_ids: ["22222222-2222-4222-8222-222222222222"] });
    const onChange = vi.fn();
    const view = render(<QueryClientProvider client={new QueryClient()}><TargetSetSelectorBuilder model={model} onChange={onChange} /></QueryClientProvider>);
    const summary = screen.getByText(field === "include_target_ids" ? /Pin hosts/ : /Exclude hosts/);
    const details = summary.closest("details");
    if (!details) throw new Error("Missing override disclosure");
    details.open = true;
    fireEvent(details, new Event("toggle"));
    const select = await within(details).findByRole("button", { name: "choose host" });
    fireEvent.click(select);
    expect(modelToSelector(onChange.mock.calls[0]?.[0])).toEqual({ name: "ordinary facet", [field]: [host.id], exclude_source_ids: ["22222222-2222-4222-8222-222222222222"] });
    fireEvent.click(within(details).getByRole("button", { name: "clear selection" }));
    expect(modelToSelector(onChange.mock.calls[1]?.[0])).not.toHaveProperty(field);
    view.unmount();
  });
});
