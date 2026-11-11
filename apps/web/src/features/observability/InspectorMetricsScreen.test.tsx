import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import InspectorMetricsScreen from "./InspectorMetricsScreen";
import type * as Api from "./api";
import { QUERY_SOURCES_KEY } from "./keys";

const api = vi.hoisted(() => ({
  listQuerySources: vi.fn(), listMetricQueries: vi.fn(), listMetricSets: vi.fn(),
  listQuerySourceAssignments: vi.fn(), listMetricSetAssignments: vi.fn(),
  listAlertSources: vi.fn(), listAlertSourceAssignments: vi.fn(), deleteQuerySource: vi.fn(),
}));
vi.mock("./api", async original => ({ ...await original<typeof Api>(), ...api }));
vi.mock("./useScopeEntities", () => ({ useScopeEntities: () => ({ resolveName: () => "Global", options: [], loading: false }) }));
// The live inspector preview has independent requests, not list or dialog state.
vi.mock("./InspectorPreviewPanel", () => ({ default: () => null }));

function mount(view = "profiles", writable = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const tree = () => <QueryClientProvider client={client}><PermissionsContext.Provider value={{
    roles: [], isAdmin: false, can: action => action === "READ" || writable, canAny: () => false, hasRole: () => false,
  }}><MemoryRouter initialEntries={[`/observability?view=${view}`]}><InspectorMetricsScreen /></MemoryRouter></PermissionsContext.Provider></QueryClientProvider>;
  const mounted = render(tree());
  return { client, setWritable: (next: boolean) => { writable = next; mounted.rerender(tree()); } };
}

beforeEach(() => {
  vi.clearAllMocks();
  Object.entries(api).forEach(([key, mock]) => { if (key.startsWith("list")) mock.mockResolvedValue({ items: [] }); });
  api.deleteQuerySource.mockResolvedValue(undefined);
});

it("keeps both empty-profile actions visible and opens their distinct assignment dialogs", async () => {
  mount();
  const empty = (await screen.findByText(/no inspector profiles/i)).parentElement!;
  fireEvent.click(within(empty).getByRole("button", { name: /^assign source$/i }));
  expect(await screen.findByRole("heading", { name: /new source assignment/i })).toBeVisible();
  fireEvent.click(within(screen.getByTestId("entity-editor-page")).getByRole("button", { name: /cancel/i }));
  fireEvent.click(within(empty).getByRole("button", { name: /^assign group$/i }));
  expect(await screen.findByRole("heading", { name: /new set assignment/i })).toBeVisible();
});

it("keeps read-only empty profiles free of assignment actions", async () => {
  mount("profiles", false);
  await screen.findByText(/no inspector profiles/i);
  expect(screen.queryByRole("button", { name: /^assign source|^assign group/i })).toBeNull();
});

it("cancels source deletion with Escape and returns focus without mutation", async () => {
  api.listQuerySources.mockResolvedValue({ items: [{ id: "source-a", name: "Source A", source_type: "prometheus", enabled: true }] });
  mount("sources");
  const trigger = await screen.findByRole("button", { name: /actions: Source A/i });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog).toHaveAccessibleDescription("Source A (source-a)");
  fireEvent.keyDown(dialog, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  await waitFor(() => expect(trigger).toHaveFocus());
  expect(api.deleteQuerySource).not.toHaveBeenCalled();
});

it("disables an open source confirmation when delete permission is revoked", async () => {
  api.listQuerySources.mockResolvedValue({ items: [{ id: "source-a", name: "Source A", source_type: "prometheus", enabled: true }] });
  const view = mount("sources");
  fireEvent.pointerDown(await screen.findByRole("button", { name: /actions: Source A/i }), { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  const dialog = await screen.findByRole("alertdialog");
  view.setWritable(false);
  const confirm = within(dialog).getByRole("button", { name: /^delete$/i });
  expect(confirm).toBeDisabled();
  fireEvent.click(confirm);
  expect(api.deleteQuerySource).not.toHaveBeenCalled();
});

it("keeps the frozen source callback after list replacement and closes before the request completes", async () => {
  api.listQuerySources.mockResolvedValue({ items: [{ id: "source-a", name: "Source A", source_type: "prometheus", enabled: true }] });
  let finish!: () => void;
  api.deleteQuerySource.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  const { client } = mount("sources");
  fireEvent.pointerDown(await screen.findByRole("button", { name: /actions: Source A/i }), { button: 0, ctrlKey: false });
  fireEvent.click(await screen.findByRole("menuitem", { name: /^delete$/i }));
  const dialog = await screen.findByRole("alertdialog");
  act(() => { client.setQueryData(QUERY_SOURCES_KEY, { items: [{ id: "source-b", name: "Source B", source_type: "prometheus", enabled: true }] }); });
  await screen.findByText("Source B");
  expect(dialog).toHaveAccessibleDescription("Source A (source-a)");
  fireEvent.click(within(dialog).getByRole("button", { name: /^delete$/i }));
  await waitFor(() => expect(api.deleteQuerySource).toHaveBeenCalledWith("source-a"));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await act(async () => { finish(); await Promise.resolve(); });
});
