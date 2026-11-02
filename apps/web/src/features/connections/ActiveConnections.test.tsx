import { act, cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import type { ActiveSession } from "@/shared/api/types";
import type * as ConnectionsApi from "./api";
import type * as AccessApi from "@/features/access-control/api";
import ActiveConnections from "./ActiveConnections";

const api = vi.hoisted(() => ({ listActiveSessions: vi.fn(), listConnectionTargets: vi.fn(), closeSession: vi.fn(), terminateSession: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConnectionsApi>(), ...api }));
vi.mock("@/features/access-control/api", async original => ({ ...await original<typeof AccessApi>(), listUsers: vi.fn().mockResolvedValue({items: []}) }));
vi.mock("@/shared/hooks/useTablePageSize", () => ({ useTablePageSize: () => ({ value: 25, setValue: vi.fn() }) }));

const session = {id: "session-id", hostId: "target-id", userId: "user-id", hostName: "Seeded target", userDisplayName: "Seeded user", protocol: "ssh", status: "active",
  connectedAt: "2026-10-05T00:00:00Z", lastActivity: "2026-10-05T00:00:00Z"} as unknown as ActiveSession;

function renderPage(readOnly = false) {
  const client = new QueryClient({defaultOptions: {queries: {retry: false}, mutations: {retry: false}}});
  render(<QueryClientProvider client={client}>{readOnly
    ? <PermissionsContext.Provider value={{roles: [], isAdmin: false, can: action => action === "READ", canAny: () => false, hasRole: () => false}}><ActiveConnections /></PermissionsContext.Provider>
    : <AllPermissions><ActiveConnections /></AllPermissions>}
  </QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  api.listActiveSessions.mockResolvedValue({items: [session], total: 1});
  api.listConnectionTargets.mockResolvedValue({items: [], total: 0});
  api.closeSession.mockResolvedValue(undefined);
  api.terminateSession.mockResolvedValue(undefined);
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("active session catalogue", () => {
  it("maps sort and array filters, resets the page and selection, and searches only fetched rows", async () => {
    api.listActiveSessions.mockResolvedValue({items: [session], total: 100});
    renderPage();
    fireEvent.click(await screen.findByText("Seeded target"));
    fireEvent.click(screen.getByRole("button", {name: "Next page"}));
    await waitFor(() => expect(api.listActiveSessions).toHaveBeenLastCalledWith(expect.objectContaining({offset: 25})));
    fireEvent.click(document.querySelector('[data-column-sort-key="lastActivity"]')!);
    await waitFor(() => expect(api.listActiveSessions).toHaveBeenLastCalledWith(expect.objectContaining({sort: "last_activity", order: "asc", offset: 0})));
    expect(screen.queryByRole("region", {name: "Session"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", {name: "Next page"}));
    await waitFor(() => expect(api.listActiveSessions).toHaveBeenLastCalledWith(expect.objectContaining({offset: 25})));
    fireEvent.click(screen.getByText("Seeded target"));
    fireEvent.click(document.querySelector('[data-column-filter-key="protocol"]')!);
    fireEvent.click(await screen.findByRole("button", {name: "SSH"}));
    fireEvent.click(screen.getByRole("button", {name: "RDP"}));
    await waitFor(() => expect(api.listActiveSessions).toHaveBeenLastCalledWith(expect.objectContaining({filters: {protocol: ["ssh", "rdp"]}, offset: 0})));
    expect(screen.queryByRole("region", {name: "Session"})).not.toBeInTheDocument();
    const count = api.listActiveSessions.mock.calls.length;
    fireEvent.change(screen.getByPlaceholderText("Search sessions..."), {target: {value: "missing"}});
    expect(screen.queryByText("Seeded target")).not.toBeInTheDocument();
    expect(api.listActiveSessions).toHaveBeenCalledTimes(count);
    expect(api.listActiveSessions.mock.lastCall?.[0]).not.toHaveProperty("search");
  });

  it("polls every five seconds and keeps the previous page while fetching", async () => {
    vi.useFakeTimers();
    api.listActiveSessions.mockResolvedValue({items: [session], total: 100});
    await act(async () => { renderPage(); await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(screen.getByText("Seeded target")).toBeInTheDocument();
    api.listActiveSessions.mockReturnValue(new Promise(() => {}));
    const count = api.listActiveSessions.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(4999); });
    expect(api.listActiveSessions).toHaveBeenCalledTimes(count);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(api.listActiveSessions).toHaveBeenCalledTimes(count + 1);
    expect(screen.getByText("Seeded target")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", {name: "Next page"}));
    expect(api.listActiveSessions).toHaveBeenLastCalledWith(expect.objectContaining({offset: 25}));
    expect(screen.getByText("Seeded target")).toBeInTheDocument();
  });
  it.each(["Disconnect", "Terminate"])("keeps %s reachable in selected details and submits the session id", async action => {
    renderPage();
    fireEvent.click(await screen.findByText("Seeded target"));
    const details = screen.getByRole("region", {name: "Session"});
    expect(within(details).getByText("Seeded user")).toBeInTheDocument();
    fireEvent.click(within(details).getByRole("button", {name: action}));
    await waitFor(() => expect(action === "Disconnect" ? api.closeSession : api.terminateSession).toHaveBeenCalledWith("session-id"));
  });

  it("refreshes the catalogue and exposes details without lifecycle actions to read-only users", async () => {
    renderPage(true);
    fireEvent.click(await screen.findByText("Seeded target"));
    const details = screen.getByRole("region", {name: "Session"});
    expect(within(details).getByRole("button", {name: "Copy Session ID"})).toBeInTheDocument();
    expect(within(details).queryByRole("button", {name: "Disconnect"})).not.toBeInTheDocument();
    expect(within(details).queryByRole("button", {name: "Terminate"})).not.toBeInTheDocument();
    fireEvent.click(within(screen.getByRole("banner")).getByRole("button", {name: "Refresh"}));
    await waitFor(() => expect(api.listActiveSessions).toHaveBeenCalledTimes(2));
    expect(api.closeSession).not.toHaveBeenCalled();
    expect(api.terminateSession).not.toHaveBeenCalled();
  });
});
