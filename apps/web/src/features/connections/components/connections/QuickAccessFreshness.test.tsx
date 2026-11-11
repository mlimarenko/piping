import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import "@/shared/i18n";
import { testUuid } from "@/test/ids";
import { connectionLaunchOptionsQueryKey, useConnectionLaunchOptions } from "@/features/connections/hooks";
import QuickAccessButtons from "./QuickAccessButtons";
import TreeView from "./TreeView";
import { DEFAULT_TREE_CONFIG } from "./treeConfig";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";

const permissions = vi.hoisted(() => ({ allowed: true }));
const fetchOptions = vi.hoisted(() => vi.fn());
vi.mock("@/features/connections/api", () => ({ getConnectionLaunchOptions: fetchOptions }));
vi.mock("@/shared/auth/permissionsContext", () => ({ usePermissions: () => ({ can: () => permissions.allowed }) }));

const host = { id: testUuid("target"), name: "target", targetKind: "device" as const, status: "active" as const };
const options = (label: string) => ({ hostId: host.id, protocols: [{ protocol: "app", render: "buttons", accounts: [{ entryId: label, kind: "shared", label, uri: `https://${label}.example.test`, source: {}, isDefault: true, requiresInput: false }] }] });
function InspectorObserver({ id = host.id }: { id?: string }) { useConnectionLaunchOptions(id); return null; }

it("refreshes cached app options once for intent and shares the in-flight request", async () => {
  let resolve!: (value: ReturnType<typeof options>) => void;
  fetchOptions.mockReset();
  fetchOptions.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(["connections", "launch-options", host.id], options("old"), { updatedAt: Date.now() - 1000 });
  render(<QueryClientProvider client={client}><InspectorObserver /><QuickAccessButtons host={host} config={{ ...DEFAULT_TREE_CONFIG.quickAccess, protocols: ["app"] }} onConnect={() => {}} /></QueryClientProvider>);
  const trigger = screen.getByTestId("quick-access-target");
  fireEvent.pointerEnter(trigger);
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  await waitFor(() => expect(fetchOptions).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole("menuitem", { name: /old/i })).not.toBeInTheDocument();
  await act(async () => { resolve(options("fresh")); await Promise.resolve(); });
  expect(await screen.findByRole("menuitem", { name: /fresh/i })).toBeInTheDocument();
  expect(fetchOptions).toHaveBeenCalledTimes(1);
});

it("warms the shared launch-options entry on host row intent before selection", async () => {
  fetchOptions.mockReset();
  fetchOptions.mockResolvedValue(options("hover"));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onSelectHost = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <TreeView
          hosts={[host]}
          selectedHost={null}
          onSelectHost={onSelectHost}
          onConnect={() => {}}
          config={DEFAULT_TREE_CONFIG}
        />
      </TooltipProvider>
    </QueryClientProvider>,
  );

  expect(fetchOptions).not.toHaveBeenCalled();
  fireEvent.pointerEnter(screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!);

  await waitFor(() => expect(fetchOptions).toHaveBeenCalledTimes(1));
  await waitFor(() =>
    expect(client.getQueryData(connectionLaunchOptionsQueryKey(host.id))).toEqual(options("hover")),
  );
  expect(onSelectHost).not.toHaveBeenCalled();
});

it("does not warm launch options on row intent without CONNECT permission", async () => {
  fetchOptions.mockReset();
  permissions.allowed = false;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  try {
    render(
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <TreeView
            hosts={[host]}
            selectedHost={null}
            onSelectHost={() => {}}
            onConnect={() => {}}
            config={DEFAULT_TREE_CONFIG}
          />
        </TooltipProvider>
      </QueryClientProvider>,
    );

    fireEvent.pointerEnter(screen.getByText(host.name).closest<HTMLElement>(".connection-tree-row")!);
    await act(async () => { await Promise.resolve(); });
    expect(fetchOptions).not.toHaveBeenCalled();
  } finally {
    permissions.allowed = true;
  }
});

it("aborts the old inspector request while its non-intentful quick-access row remains mounted", async () => {
  const signals = new Map<string, AbortSignal>();
  fetchOptions.mockReset();
  fetchOptions.mockImplementation((id: string, signal: AbortSignal) => {
    signals.set(id, signal);
    return new Promise(() => {});
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const row = <QuickAccessButtons host={host} config={{ ...DEFAULT_TREE_CONFIG.quickAccess, protocols: ["ssh"] }} onConnect={() => {}} />;
  const view = render(<QueryClientProvider client={client}>{row}<InspectorObserver /></QueryClientProvider>);
  await waitFor(() => expect(signals.has(host.id)).toBe(true));
  view.rerender(<QueryClientProvider client={client}>{row}<InspectorObserver id="other" /></QueryClientProvider>);
  await waitFor(() => expect(signals.get(host.id)?.aborted).toBe(true));
  expect(signals.get("other")?.aborted).toBe(false);
});
