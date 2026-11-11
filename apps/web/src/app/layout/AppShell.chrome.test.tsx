import { lazy, useEffect, useState, type ReactNode } from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import AppShell from "./AppShell";
import PageChrome from "@/shared/ui-shared/PageChrome";
import type * as Settings from "@/shared/settings";
import type * as I18n from "@/shared/i18n";

const lifetime = vi.hoisted(() => ({ mount: vi.fn(), unmount: vi.fn() }));
vi.mock("./AppSidebar", () => ({ default: () => <aside>Navigation</aside> }));
vi.mock("./StatusBar", () => ({ default: () => <footer>Status</footer> }));
vi.mock("./UserMenu", () => ({ default: () => <button>User menu</button> }));
vi.mock("@/app/BrandingContext", () => ({ useBranding: () => ({ appName: "Piping" }) }));
vi.mock("@/features/assistant", () => ({ AssistantProvider: ({ children }: { children: ReactNode }) => children, AssistantWindow: () => null }));
vi.mock("@/shared/hooks/useUiScalePreference", () => ({ useUiScalePreference: () => ({ value: 1 }), useApplyUiScale: vi.fn() }));
vi.mock("@/shared/hooks/theme-context", () => ({ useTheme: () => ({ resolvedTheme: "light", setTheme: vi.fn() }) }));
vi.mock("@/shared/settings", async original => ({ ...await original<typeof Settings>(), useSetting: () => ({ setValue: vi.fn() }) }));
vi.mock("@/shared/i18n", async original => ({ ...await original<typeof I18n>(), loadLanguagePreference: vi.fn() }));
vi.mock("@/features/connections/ConnectionsWorkspace", () => ({ default: function Workspace({ visible }: { visible: boolean }) {
  useEffect(() => { lifetime.mount(); return () => lifetime.unmount(); }, []);
  return <><PageChrome title="Connections" visible={visible} actions={<button>Connection action</button>} /><input aria-label="Session buffer" defaultValue="retained output" /></>;
} }));
beforeEach(() => { lifetime.mount.mockClear(); lifetime.unmount.mockClear(); });

it("keeps the shell and connection workspace mounted while a lazy routed page loads", async () => {
  let resolve!: (module: { default: () => ReactNode }) => void;
  const Next = lazy(() => new Promise<{ default: () => ReactNode }>(done => { resolve = done; }));
  function Navigation() {
    const navigate = useNavigate();
    return <><button onClick={() => void navigate("/next")}>Next route</button><button onClick={() => void navigate("/connections")}>Return</button></>;
  }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["auth", "me"], { id: "test-user", capabilities: [{ action: "READ", resourceType: "CONNECTION_TARGET" }] });
  render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/connections"]}><Navigation /><Routes><Route element={<AppShell />}>
    <Route path="/connections" element={null} /><Route path="/next" element={<Next />} />
  </Route></Routes></MemoryRouter></QueryClientProvider>);
  const buffer = await screen.findByRole("textbox", { name: "Session buffer" });
  const header = screen.getByRole("banner");
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Connections");
  fireEvent.change(buffer, { target: { value: "unsent output" } });
  fireEvent.click(screen.getByRole("button", { name: "Next route" }));
  expect(screen.getByRole("banner")).toBe(header);
  await act(async () => { resolve({ default: () => <><PageChrome title="Next" actions={<button>Next action</button>} /><p>Next page body</p></> }); await Promise.resolve(); });
  expect(await screen.findByText("Next page body")).toBeVisible();
  expect(screen.getByRole("banner")).toBe(header);
  expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Next");
  expect(screen.queryByRole("button", { name: "Connection action" })).toBeNull();
  expect(buffer.closest("[hidden][inert]")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Return" }));
  expect(await screen.findByRole("textbox", { name: "Session buffer" })).toBe(buffer);
  expect(buffer).toHaveValue("unsent output");
  expect(lifetime.mount).toHaveBeenCalledTimes(1);
  expect(lifetime.unmount).not.toHaveBeenCalled();
});

function renderErrorRoute(Page: () => ReactNode) {
  function Navigation() {
    const navigate = useNavigate();
    return <><button onClick={() => void navigate("/next")}>Next route</button><button onClick={() => void navigate("/connections")}>Return</button></>;
  }
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["auth", "me"], { id: "test-user", capabilities: [{ action: "READ", resourceType: "CONNECTION_TARGET" }] });
  return render(<QueryClientProvider client={client}><MemoryRouter initialEntries={["/connections"]}><Navigation /><Routes><Route element={<AppShell />}>
    <Route path="/connections" element={null} /><Route path="/next" element={<Page />} />
  </Route></Routes></MemoryRouter></QueryClientProvider>);
}
it("owns explicit error chrome, clears stale actions and recovers a render failure on Retry without remounting the workspace", async () => {
  let fail = true;
  function Page() {
    const [broken, setBroken] = useState(false);
    if (broken && fail) throw new Error("Fixture render failure");
    return <><PageChrome title="Inventory" actions={<button onClick={() => setBroken(true)}>Crash page</button>} /><p>Inventory body</p></>;
  }
  const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    renderErrorRoute(Page);
    const buffer = await screen.findByRole("textbox", { name: "Session buffer" });
    const header = screen.getByRole("banner");
    fireEvent.click(screen.getByRole("button", { name: "Next route" }));
    fireEvent.click(await screen.findByRole("button", { name: "Crash page" }));
    const alert = await screen.findByRole("alert");
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Failed to load data");
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("button", { name: "Crash page" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Connection action" })).toBeNull();
    fail = false;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Inventory body")).toBeVisible();
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Inventory");
    expect(screen.getByRole("banner")).toBe(header);
    fireEvent.click(screen.getByRole("button", { name: "Return" }));
    expect(await screen.findByRole("textbox", { name: "Session buffer" })).toBe(buffer);
    expect(lifetime.mount).toHaveBeenCalledTimes(1);
    expect(lifetime.unmount).not.toHaveBeenCalled();
  } finally { errorLog.mockRestore(); }
});
it("owns explicit error chrome for a rejected lazy import and recovers on navigation with the same retained workspace", async () => {
  const Broken = lazy(() => Promise.reject<{ default: () => ReactNode }>(new Error("Fixture import rejection")));
  const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    renderErrorRoute(() => <Broken />);
    const buffer = await screen.findByRole("textbox", { name: "Session buffer" });
    const header = screen.getByRole("banner");
    fireEvent.click(screen.getByRole("button", { name: "Next route" }));
    await screen.findByRole("alert");
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Failed to load data");
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("button", { name: "Connection action" })).toBeNull();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Return" }));
    expect(await screen.findByRole("textbox", { name: "Session buffer" })).toBe(buffer);
    expect(screen.getByRole("banner")).toBe(header);
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Connections");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(lifetime.mount).toHaveBeenCalledTimes(1);
    expect(lifetime.unmount).not.toHaveBeenCalled();
  } finally { errorLog.mockRestore(); }
});
