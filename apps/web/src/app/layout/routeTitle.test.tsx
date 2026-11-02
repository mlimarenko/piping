import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import type { ReactNode } from "react";
import { expect, it, vi } from "vitest";
import AppShell from "./AppShell";

vi.mock("./StatusBar", () => ({ default: () => null }));
vi.mock("./UserMenu", () => ({ default: () => null }));
vi.mock("@/app/BrandingContext", () => ({ useBranding: () => ({ appName: "Test instance" }) }));
vi.mock("@/features/assistant", () => ({ AssistantProvider: ({ children }: { children: ReactNode }) => children, AssistantWindow: () => null, AssistantLauncher: () => null }));
vi.mock("@/shared/hooks/useUiScalePreference", () => ({ useUiScalePreference: () => ({ value: 1 }), useApplyUiScale: vi.fn() }));
vi.mock("@/shared/hooks/theme-context", () => ({ useTheme: () => ({ resolvedTheme: "light", setTheme: vi.fn() }) }));
vi.mock("@/features/connections/ConnectionsWorkspace", () => ({ default: () => null }));

it.each([
  ["/connections/proxies/node-1/edit", "Administration"],
  ["/automation/schedules/schedule-1/edit", "Automation"],
  ["/automation/projects/project-1/files", "Automation"],
  ["/playbooks/playbook-1/edit", "Automation"],
  ["/playbooks/11111111-1111-4111-8111-111111111111", "Automation"],
  ["/automation/runs", "Automation"],
  ["/api-docs", "Administration"],
  ["/user/settings/credentials/credential-1/edit", "Account"],
])("uses canonical ownership for shell title and sidebar on %s", async (path, section) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["auth", "me"], { id: "reader", username: "reader", roles: [], capabilities: [{ action: "READ", resourceType: "CONNECTION_TARGET" }] });
  const view = render(<QueryClientProvider client={client}><MemoryRouter initialEntries={[path]}><Routes>
    <Route element={<AppShell />}><Route path="*" element={<p>Route body</p>} /></Route>
  </Routes></MemoryRouter></QueryClientProvider>);
  await screen.findByText("Route body");
  await waitFor(() => expect(document.title).toBe(`Test instance · ${section}`));
  // Even when the destination is hidden/denied, a broader visible catalogue
  // must not claim its active item. Title ownership is independent of access.
  const active = screen.getByRole("navigation").querySelector('[aria-current="page"]');
  if (path.startsWith("/user/")) expect(active).toHaveAttribute("href", "/user/settings");
  else expect(active).toBeNull();
  if (path === "/api-docs") expect(screen.queryByRole("link", { name: /api/i })).toBeNull();
  view.unmount();
  client.clear();
});
