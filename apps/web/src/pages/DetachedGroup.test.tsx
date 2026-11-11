import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { beforeEach, expect, it, vi } from "vitest";
import DetachedGroup from "./DetachedGroup";
import { readDetachedHandoff, writeDetachedHandoff } from "@/features/connections/detachedHandoff";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

const mocks = vi.hoisted(() => ({ open: vi.fn(), close: vi.fn(), options: vi.fn(), resume: vi.fn() }));
vi.mock("@/features/connections/api", () => ({ openSession: mocks.open, closeSession: mocks.close, getConnectionLaunchOptions: mocks.options, resumeSession: mocks.resume }));
vi.mock("@/features/connections/components/connections/ConnectionsPanel", () => ({
  default: ({ sessions, activeSessionId, onReconnectSession, onSelectSession }: { sessions: LiveSession[]; activeSessionId: string; onReconnectSession: (session: LiveSession) => void; onSelectSession: (id: string) => void }) => <div>
    <p data-testid="active">{activeSessionId}</p>
    {sessions.map(session => <div key={session.id} data-testid={session.id}><button onClick={() => onReconnectSession(session)}>Retry {session.id}</button><button onClick={() => onSelectSession(session.id)}>Select {session.id}</button></div>)}
  </div>,
}));

beforeEach(() => { vi.clearAllMocks(); window.localStorage.clear(); mocks.close.mockResolvedValue(undefined); });

it("fresh-launches a group Retry, preserves other tabs/focus and updates handoff identity", async () => {
  for (const id of ["old-a", "live-b"]) writeDetachedHandoff(id, { sessionId: id, hostId: id, hostName: id, protocol: "ssh", websocketUrl: `/ssh/${id}`, token: "fixture" });
  mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ entryId: "entry", kind: "shared", label: "Operator", requiresInput: false }] }] });
  mocks.open.mockResolvedValue({ session: { id: "new-a", hostId: "old-a", protocol: "ssh" }, proxy: { websocketUrl: "/ssh/new-a", token: "fixture" } });
  render(<QueryClientProvider client={new QueryClient()}><MemoryRouter initialEntries={["/connections/detached/group?ids=old-a,live-b"]}><DetachedGroup /></MemoryRouter></QueryClientProvider>);
  const b = screen.getByTestId("live-b");
  fireEvent.click(screen.getByRole("button", { name: "Select live-b" }));
  fireEvent.click(screen.getByRole("button", { name: "Retry old-a" }));
  await screen.findByTestId("new-a");
  await waitFor(() => expect(mocks.close).toHaveBeenCalledExactlyOnceWith("old-a"));
  expect(mocks.resume).not.toHaveBeenCalled();
  expect(screen.getByTestId("live-b")).toBe(b);
  expect(screen.getByTestId("active")).toHaveTextContent("live-b");
  expect(readDetachedHandoff("old-a")).toBeNull();
  expect(readDetachedHandoff("new-a")?.sessionId).toBe("new-a");
});
