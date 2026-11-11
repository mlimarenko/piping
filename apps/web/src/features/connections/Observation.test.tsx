import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import "@/shared/i18n";
import type * as ConnectionsApi from "./api";
import type * as AccessApi from "@/features/access-control/api";
import { readObservationHandoff, removeObservationHandoff } from "@/features/connections/observation";
import ActiveConnections from "./ActiveConnections";

const mocks = vi.hoisted(() => ({ listActiveSessions: vi.fn(), listConnectionTargets: vi.fn(), closeSession: vi.fn(), terminateSession: vi.fn(), viewerUnmount: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConnectionsApi>(), ...mocks }));
vi.mock("@/features/access-control/api", async original => ({ ...await original<typeof AccessApi>(), listUsers: vi.fn().mockResolvedValue({items: []}) }));
vi.mock("@/shared/hooks/useTablePageSize", () => ({ useTablePageSize: () => ({ value: 25, setValue: vi.fn() }) }));
vi.mock("@/features/connections/components/connections/LiveSessionViewer", async () => {
  const React = await import("react");
  function MockObserver(props: {readOnly?: boolean; websocketUrl: string; token: string}) {
    React.useEffect(() => () => mocks.viewerUnmount(), []);
    return <div data-testid="observer-viewer">{String(props.readOnly)} {props.websocketUrl} {props.token}</div>;
  }
  return { default: MockObserver };
});

const baseSession = {id: "session-id", hostId: "target-id", userId: "user-id", hostName: "Target", protocol: "ssh", status: "active", connectedAt: "2026-10-05T00:00:00Z", lastActivity: "2026-10-05T00:00:00Z"};
function show() {
  const client = new QueryClient({defaultOptions: {queries: {retry: false}}});
  render(<MemoryRouter><QueryClientProvider client={client}><AllPermissions><ActiveConnections /></AllPermissions></QueryClientProvider></MemoryRouter>);
}
beforeEach(() => { vi.clearAllMocks(); mocks.listConnectionTargets.mockResolvedValue({items: []}); removeObservationHandoff("session-id"); });
afterEach(cleanup);

describe("session observation", () => {
  it("opens details from a button, restores focus, and explains server denial", async () => {
    mocks.listActiveSessions.mockResolvedValue({items: [{...baseSession, observe: {allowed: false, reason: "permission_denied", websocket_url: null, transport: null}}], total: 1});
    show();
    const trigger = await screen.findByRole("button", {name: "Target"});
    trigger.focus(); fireEvent.click(trigger);
    expect(screen.getByRole("heading", {name: "Session"})).toHaveFocus();
    expect(screen.getByRole("button", {name: "Observe"})).toBeDisabled();
    expect(screen.getByText("You do not have permission to observe this session.")).toBeInTheDocument();
    expect(screen.queryByText("permission_denied")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", {name: "Close details"}));
    expect(trigger).toHaveFocus();
  });

  it("opens details with the table keyboard action and restores row focus", async () => {
    mocks.listActiveSessions.mockResolvedValue({items: [baseSession], total: 1});
    show();
    const row = (await screen.findByRole("button", {name: "Target"})).closest("tr")!;
    row.focus();
    fireEvent.keyDown(row, {key: "Enter"});
    expect(screen.getByRole("heading", {name: "Session"})).toHaveFocus();
    fireEvent.click(screen.getByRole("button", {name: "Close details"}));
    expect(row).toHaveFocus();
  });

  it("opens a capability-approved read-only viewer and closes without lifecycle calls", async () => {
    mocks.listActiveSessions.mockResolvedValue({items: [{...baseSession, observe: {allowed: true, reason: null, websocket_url: "/api/v1/ws/observe/session-id", transport: "terminal"}}], total: 1});
    show(); fireEvent.click(await screen.findByRole("button", {name: "Target"}));
    fireEvent.click(screen.getByRole("button", {name: "Observe"}));
    expect(screen.getByTestId("observer-viewer")).toHaveTextContent("true ws://localhost:3000/api/v1/ws/observe/session-id");
    fireEvent.click(screen.getByRole("button", {name: "Close viewer"}));
    await waitFor(() => expect(mocks.viewerUnmount).toHaveBeenCalledOnce());
    expect(mocks.closeSession).not.toHaveBeenCalled();
    expect(mocks.terminateSession).not.toHaveBeenCalled();
  });

  it("rejects an allowed capability pointing to a different origin", async () => {
    mocks.listActiveSessions.mockResolvedValue({items: [{...baseSession, observe: {allowed: true, reason: null, websocket_url: "wss://attacker.example/api/v1/ws/observe/session-id", transport: "terminal"}}], total: 1});
    show(); fireEvent.click(await screen.findByRole("button", {name: "Target"}));
    expect(screen.getByRole("button", {name: "Observe"})).toBeDisabled();
  });

  it("opens the observation in a separate window and leaves one observer stream", async () => {
    mocks.listActiveSessions.mockResolvedValue({items: [{...baseSession, observe: {allowed: true, reason: null, websocket_url: "/api/v1/ws/observe/session-id", transport: "terminal"}}], total: 1});
    const open = vi.spyOn(window, "open").mockReturnValue({} as Window);
    show();
    fireEvent.click(await screen.findByRole("button", {name: "Target"}));
    fireEvent.click(screen.getByRole("button", {name: "Observe"}));
    expect(screen.getByTestId("observer-viewer")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", {name: "Open in new window"}));

    expect(open).toHaveBeenCalledWith("/connections/observe/session-id", "_blank");
    await waitFor(() => expect(mocks.viewerUnmount).toHaveBeenCalledOnce());
    expect(readObservationHandoff("session-id")).toMatchObject({
      sessionId: "session-id",
      hostName: "Target",
      protocol: "ssh",
      websocketUrl: "ws://localhost:3000/api/v1/ws/observe/session-id",
    });
    open.mockRestore();
  });
});
