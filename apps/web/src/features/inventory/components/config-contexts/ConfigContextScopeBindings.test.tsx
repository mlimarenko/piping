import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import { AllPermissions } from "@/test/permissions";
import ConfigContextScopeBindings from "./ConfigContextScopeBindings";
import type * as configContextsApi from "@/features/inventory/config-contexts-api";
import type * as inventoryApi from "@/features/inventory/api";

const CONTEXT_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const TAG_ID = "11111111-1111-4111-8111-111111111111";
const SITE_ID = "22222222-2222-4222-8222-222222222222";
const SITE_B_ID = "33333333-3333-4333-8333-333333333333";

const BINDING_A = { id: "binding-a", context_id: CONTEXT_ID, scope_type: "site", scope_id: SITE_ID };
const BINDING_B = { id: "binding-b", context_id: CONTEXT_ID, scope_type: "site", scope_id: SITE_B_ID };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

const mocks = vi.hoisted(() => ({
  listBindings: vi.fn(),
  createBinding: vi.fn(),
  deleteBinding: vi.fn(),
  listAllResources: vi.fn(),
}));
vi.mock("@/features/inventory/config-contexts-api", async (original) => ({
  ...await original<typeof configContextsApi>(),
  listBindings: mocks.listBindings,
  createBinding: mocks.createBinding,
  deleteBinding: mocks.deleteBinding,
}));
vi.mock("@/features/inventory/api", async (original) => ({
  ...await original<typeof inventoryApi>(),
  listAllResources: mocks.listAllResources,
}));

function mount(contextActive = true) {  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <ConfigContextScopeBindings contextId={CONTEXT_ID} contextActive={contextActive} />
      </AllPermissions>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listAllResources.mockImplementation((entity: string) =>
    Promise.resolve(entity === "Tags"
      ? [{ id: TAG_ID, name: "Production", type: "tag", color: "ff0000" }]
      : entity === "Sites"
        ? [
            { id: SITE_ID, name: "Site A", type: "site" },
            { id: SITE_B_ID, name: "Site B", type: "site" },
          ]
        : []),
  );
});

describe("ConfigContextScopeBindings", () => {
  it("adds a tag binding picked from the Tags catalogue", async () => {
    mocks.listBindings.mockResolvedValue({ items: [], total: 0 });
    mount();

    await screen.findByText(en.configContexts.scope.appliesGlobally);
    fireEvent.click(screen.getByRole("combobox", { name: en.configContexts.scope.scopeType }));
    fireEvent.click(screen.getByRole("option", { name: "Tag" }));

    const target = screen.getByRole("combobox", { name: en.configContexts.scope.target });
    await waitFor(() => expect(target).toBeEnabled());
    fireEvent.click(target);
    fireEvent.click(await screen.findByRole("option", { name: "Production" }));
    fireEvent.click(screen.getByRole("button", { name: en.add }));

    await waitFor(() =>
      expect(mocks.createBinding).toHaveBeenCalledWith({
        context_id: CONTEXT_ID,
        scope_type: "tag",
        scope_id: TAG_ID,
      }),
    );
  });

  it("warns that an active context without bindings applies globally", async () => {
    mocks.listBindings.mockResolvedValue({ items: [], total: 0 });
    mount();

    expect(await screen.findByText(en.configContexts.scope.appliesGlobally)).toBeInTheDocument();
    expect(screen.queryByText(en.configContexts.scope.globalWhenActive)).not.toBeInTheDocument();
  });

  it("does not claim global application for an inactive context", async () => {
    mocks.listBindings.mockResolvedValue({ items: [], total: 0 });
    mount(false);

    expect(await screen.findByText(en.configContexts.scope.globalWhenActive)).toBeInTheDocument();
    expect(screen.queryByText(en.configContexts.scope.appliesGlobally)).not.toBeInTheDocument();
  });

  it("keeps the last binding when the global-change confirmation is cancelled", async () => {
    mocks.listBindings.mockResolvedValue({
      items: [{ id: "binding", context_id: CONTEXT_ID, scope_type: "site", scope_id: SITE_ID }],
      total: 1,
    });
    mount();
    await screen.findByText("Site A");

    const trigger = screen.getByRole("button", { name: /Actions:.*Site A/ });
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: en.remove }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: en.cancel }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(mocks.deleteBinding).not.toHaveBeenCalled();
  });

  it("cannot skip the global confirmation while the post-delete count is stale", async () => {
    const refetch = deferred<{ items: Array<{ id: string; context_id: string; scope_type: string; scope_id: string }>; total: number }>();
    mocks.listBindings
      .mockResolvedValueOnce({ items: [BINDING_A, BINDING_B], total: 2 })
      .mockImplementationOnce(() => refetch.promise);
    mount();
    await screen.findByText("Site A");

    const removeA = screen.getByRole("button", { name: /Actions:.*Site A/ });
    fireEvent.keyDown(removeA, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: en.remove }));
    await waitFor(() => expect(mocks.deleteBinding).toHaveBeenCalledWith("binding-a"));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

    // The authoritative total is still 2 (refetch pending): the second
    // removal must stay unavailable instead of bypassing the confirmation.
    const removeB = screen.getByRole("button", { name: /Actions:.*Site B/ });
    fireEvent.keyDown(removeB, { key: "Enter" });
    const pendingRemove = await screen.findByRole("menuitem", { name: en.remove });
    expect(pendingRemove).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(pendingRemove);
    expect(mocks.deleteBinding).toHaveBeenCalledTimes(1);

    refetch.resolve({
      items: [BINDING_B],
      total: 1,
    });
    await waitFor(() => expect(pendingRemove).not.toHaveAttribute("aria-disabled", "true"));
    fireEvent.click(pendingRemove);

    const dialog = await screen.findByRole("alertdialog");
    expect(mocks.deleteBinding).toHaveBeenCalledTimes(1);
    fireEvent.click(within(dialog).getByRole("button", { name: en.configContexts.scope.removeLastConfirm }));
    await waitFor(() => expect(mocks.deleteBinding).toHaveBeenCalledWith("binding-b"));
  });

  it("blocks further removals when the post-delete count refresh fails", async () => {
    mocks.listBindings
      .mockResolvedValueOnce({ items: [BINDING_A, BINDING_B], total: 2 })
      .mockRejectedValueOnce(new Error("refresh failed"));
    mount();
    await screen.findByText("Site A");

    const removeA = screen.getByRole("button", { name: /Actions:.*Site A/ });
    fireEvent.keyDown(removeA, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: en.remove }));
    await waitFor(() => expect(mocks.deleteBinding).toHaveBeenCalledWith("binding-a"));

    // Fail closed: without a fresh total the rows are replaced by the query
    // error state, so no unconfirmed second removal is reachable.
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions:.*Site B/ })).not.toBeInTheDocument();
    expect(mocks.deleteBinding).toHaveBeenCalledTimes(1);
  });
});
