import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import { CONNECTION_TARGETS_QUERY_KEY } from "@/features/connections/api";
import { useScopeEntities } from "./useScopeEntities";

const api = vi.hoisted(() => ({ listAllResources: vi.fn(), listConnectionTargets: vi.fn() }));
vi.mock("@/features/inventory/api", () => ({ listAllResources: api.listAllResources }));
vi.mock("@/features/connections/api", () => ({ CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"], listConnectionTargets: api.listConnectionTargets }));
beforeEach(() => {
  vi.resetAllMocks();
  api.listAllResources.mockImplementation(() => Promise.resolve(Array.from({ length: 1001 }, (_, index) => ({ id: `dict-${index}`, name: `Synthetic dictionary ${index}` }))));
  api.listConnectionTargets.mockResolvedValue({ items: [
    { id: "target-first", name: "Synthetic duplicate", targetKind: "device", primaryIp: "192.0.2.1" },
    { id: "target-last", name: "Synthetic duplicate", targetKind: "virtual_machine", primaryIp: "192.0.2.2" },
  ] });
});
function setup(includeConnectionTargets = false, allowed = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: () => allowed, canAny: () => allowed, hasRole: () => false }}>{children}</PermissionsContext.Provider></QueryClientProvider>;
  const hook = renderHook(() => useScopeEntities("All matching targets", { includeConnectionTargets }), { wrapper });
  return { ...hook, client, revoke: () => { allowed = false; hook.rerender(); } };
}
it("resolves dictionaries after the old first-1000 bound without requesting target inventory", async () => {
  const { result } = setup();
  await waitFor(() => expect(result.current.resolveName("site", "dict-1000")).toBe("Synthetic dictionary 1000"));
  expect(api.listAllResources).toHaveBeenCalledTimes(3);
  expect(api.listConnectionTargets).not.toHaveBeenCalled();
});
it("uses the canonical complete target cache and disambiguates duplicate names", async () => {
  const { result, client, revoke } = setup(true);
  await waitFor(() => expect(result.current.resolveName("connection_target", "target-last")).toContain("192.0.2.2"));
  expect(result.current.resolveName("connection_target", "target-first")).toContain("device");
  expect(client.getQueryData(CONNECTION_TARGETS_QUERY_KEY)).toBeDefined();
  expect(result.current.optionsByScope.connection_target).toEqual([]);
  revoke();
  expect(result.current.resolveName("connection_target", "target-last")).toBe("Name unavailable · target-last");
});
it("does not request denied target names and treats lookup failure as unavailable", async () => {
  const denied = setup(true, false);
  expect(denied.result.current.resolveName("connection_target", "target-first")).toContain("Name unavailable");
  expect(api.listConnectionTargets).not.toHaveBeenCalled();
  denied.unmount();
  api.listConnectionTargets.mockRejectedValue(new Error("denied fixture"));
  const failed = setup(true);
  await act(async () => { await failed.client.invalidateQueries({ queryKey: CONNECTION_TARGETS_QUERY_KEY }); });
  await waitFor(() => expect(api.listConnectionTargets).toHaveBeenCalled());
  expect(failed.result.current.resolveName("connection_target", "target-first")).toBe("Name unavailable · target-first");
});
