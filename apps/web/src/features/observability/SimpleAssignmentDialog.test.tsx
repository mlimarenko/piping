import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import SimpleAssignmentDialog, { type AssignmentDraft } from "./SimpleAssignmentDialog";
import type { ScopeEntities } from "./useScopeEntities";

const initial: AssignmentDraft = { targetId: "first", scopeType: "global", scopeId: "", weight: "0", enabled: true };
const entities: ScopeEntities = {
  optionsByScope: { role: [], connection_target: [], site: [], tenant: [] },
  resolveName: () => "",
  isPending: false,
};

describe("monitoring assignment editor", () => {
  it.each(["Data source", "Metric set", "Alert source"])("associates the %s picker and protects its pending draft", async (targetLabel) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onOpenChange = vi.fn();
    const onSubmit = vi.fn();
    const page = (pending: boolean) => (
      <QueryClientProvider client={client}>
        <SimpleAssignmentDialog open onOpenChange={onOpenChange} isEditing={false}
          title="New assignment" description="Bind a monitoring entity to a scope"
          targetLabel={targetLabel} targetOptions={[{ id: "first", name: "First" }, { id: "second", name: "Second" }]}
          initial={initial} entities={entities} pending={pending} onSubmit={onSubmit} />
      </QueryClientProvider>
    );
    const { rerender } = render(page(false));
    const picker = screen.getByRole("combobox", { name: new RegExp(`^${targetLabel}\\b`) });
    fireEvent.click(picker);
    const second = await screen.findByRole("option", { name: /Second/ });
    rerender(page(true));
    expect(picker).toBeDisabled();
    fireEvent.click(second);
    expect(picker).toHaveTextContent("First");
    expect(screen.getByRole("button", { name: /^Create$/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /^Cancel$/i })).toBeDisabled();
    expect(onSubmit).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();
    rerender(page(false));
    await waitFor(() => expect(picker).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /^Create$/i }));
    expect(onSubmit).toHaveBeenCalledWith(initial);
  });
});
