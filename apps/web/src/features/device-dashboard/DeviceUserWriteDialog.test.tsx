import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DeviceUserWriteDialog from "./DeviceUserWriteDialog";
import type { PendingDeviceUserWrite } from "./deviceUserWrites";
import type { DeviceUsersOutcome, DeviceUsersPreview } from "./api";
import { ApiError } from "@/shared/api/client";

const api = vi.hoisted(() => ({ previewDeviceUsers: vi.fn(), createDeviceUsers: vi.fn(), deleteDeviceUsers: vi.fn() }));
vi.mock("./api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

beforeEach(() => {
  Object.values(api).forEach(mock => mock.mockReset());
  api.previewDeviceUsers.mockResolvedValue({ writes_enabled: true, per_item: [{ name: "operator", action: "delete", reason: "Remove account" }] });
});

it("submits on target B while target A is pending and keeps B locked after A settles", async () => {
  let finishA!: (outcome: DeviceUsersOutcome) => void;
  let finishB!: (outcome: DeviceUsersOutcome) => void;
  api.deleteDeviceUsers.mockReturnValueOnce(new Promise<DeviceUsersOutcome>(resolve => { finishA = resolve; }))
    .mockReturnValueOnce(new Promise<DeviceUsersOutcome>(resolve => { finishB = resolve; }));
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const action: PendingDeviceUserWrite = { kind: "delete", nonce: 1, title: "Remove operator", usernames: ["operator"] };
  const view = (pending: PendingDeviceUserWrite | null, targetId: string) => <QueryClientProvider client={client}>
    <DeviceUserWriteDialog pending={pending} targetId={targetId} onClose={() => undefined} onApplied={() => undefined} onWritesDisabled={() => undefined} />
  </QueryClientProvider>;
  const rendered = render(view(action, "target-a"));
  await waitFor(() => expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(api.deleteDeviceUsers).toHaveBeenCalledTimes(1));
  rendered.rerender(view(null, "target-a"));
  rendered.rerender(view({ ...action, nonce: 2 }, "target-b"));
  await waitFor(() => expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(api.deleteDeviceUsers).toHaveBeenCalledTimes(2), { timeout: 1000 });
  expect(api.deleteDeviceUsers).toHaveBeenNthCalledWith(2, "target-b", ["operator"]);
  await act(async () => { finishA({ succeeded: ["operator"], failed: [], all_succeeded: true }); await Promise.resolve(); });
  expect(screen.getByRole("button", { name: /confirm/i })).toBeDisabled();
  await act(async () => { finishB({ succeeded: ["operator"], failed: [], all_succeeded: true }); await Promise.resolve(); });
  await waitFor(() => expect(screen.getByRole("button", { name: /close/i })).toBeEnabled());
});

it("ignores a late permission denial from another action generation", async () => {
  let fail!: (error: Error) => void;
  api.deleteDeviceUsers.mockReturnValue(new Promise<DeviceUsersOutcome>((_, reject) => { fail = reject; }));
  const onWritesDisabled = vi.fn();
  const onApplied = vi.fn();
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const pending: PendingDeviceUserWrite = { kind: "delete", nonce: 1, title: "Remove operator", usernames: ["operator"] };
  const view = (action: PendingDeviceUserWrite | null, targetId = "target-1") => <QueryClientProvider client={client}>
    <DeviceUserWriteDialog pending={action} targetId={targetId} onClose={() => undefined} onApplied={onApplied} onWritesDisabled={onWritesDisabled} />
  </QueryClientProvider>;
  const rendered = render(view(pending));
  await waitFor(() => expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(api.deleteDeviceUsers).toHaveBeenCalledTimes(1));
  rendered.rerender(view(null));
  rendered.rerender(view({ ...pending, nonce: 2 }, "target-2"));
  await waitFor(() => expect(api.previewDeviceUsers).toHaveBeenCalledTimes(2));
  await act(async () => {
    fail(new ApiError({ status: 403, message: "Denied" }));
    await Promise.resolve();
  });
  expect(onWritesDisabled).not.toHaveBeenCalled();
  expect(onApplied).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled();
});

it("does not let a late apply affect a reopened action or allow overlapping writes", async () => {
  let finish!: (outcome: DeviceUsersOutcome) => void;
  api.deleteDeviceUsers.mockReturnValue(new Promise<DeviceUsersOutcome>(resolve => { finish = resolve; }));
  const onApplied = vi.fn();
  const onWritesDisabled = vi.fn();
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const actionA: PendingDeviceUserWrite = { kind: "delete", nonce: 1, title: "Action A", usernames: ["operator"] };
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const actionB: PendingDeviceUserWrite = { kind: "create", nonce: 2, title: "Action B", users: [{ name: "other" }] };
  const view = (pending: PendingDeviceUserWrite | null) => <QueryClientProvider client={client}>
    <DeviceUserWriteDialog pending={pending} targetId="target-1" onClose={() => undefined} onApplied={onApplied} onWritesDisabled={onWritesDisabled} />
  </QueryClientProvider>;
  const rendered = render(view(actionA));
  await waitFor(() => expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: /confirm/i }));
  await waitFor(() => expect(api.deleteDeviceUsers).toHaveBeenCalledTimes(1));
  rendered.rerender(view(null));
  rendered.rerender(view(actionB));
  await waitFor(() => expect(api.previewDeviceUsers).toHaveBeenCalledTimes(2));
  const dialog = screen.getByRole("dialog", { name: "Action B" });
  expect(within(dialog).getByRole("button", { name: /confirm/i })).toBeDisabled();
  await act(async () => {
    finish({ succeeded: ["operator"], failed: [], all_succeeded: true });
    await Promise.resolve();
  });
  expect(onApplied).not.toHaveBeenCalled();
  expect(onWritesDisabled).not.toHaveBeenCalled();
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["device", "target-1", "users"] });
  await waitFor(() => expect(within(dialog).getByRole("button", { name: /confirm/i })).toBeEnabled());
  expect(within(dialog).queryByText(/succeeded/i)).not.toBeInTheDocument();
});

it("keeps the new preview when a superseded read reports writes disabled", async () => {
  let finish!: (preview: DeviceUsersPreview) => void;
  api.previewDeviceUsers.mockReturnValueOnce(new Promise<DeviceUsersPreview>(resolve => { finish = resolve; }));
  const onWritesDisabled = vi.fn();
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const actionA: PendingDeviceUserWrite = { kind: "delete", nonce: 1, title: "Action A", usernames: ["operator"] };
  const actionB: PendingDeviceUserWrite = { ...actionA, nonce: 2, title: "Action B" };
  const view = (pending: PendingDeviceUserWrite | null) => <QueryClientProvider client={client}>
    <DeviceUserWriteDialog pending={pending} targetId="target-1" onClose={() => undefined} onApplied={() => undefined} onWritesDisabled={onWritesDisabled} />
  </QueryClientProvider>;
  const rendered = render(view(actionA));
  await waitFor(() => expect(api.previewDeviceUsers).toHaveBeenCalledTimes(1));
  rendered.rerender(view(null));
  rendered.rerender(view(actionB));
  await waitFor(() => expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled());
  await act(async () => {
    finish({ writes_enabled: false, per_item: [] });
    await Promise.resolve();
  });
  expect(onWritesDisabled).not.toHaveBeenCalled();
  expect(screen.getByRole("dialog", { name: "Action B" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /confirm/i })).toBeEnabled();
  expect(api.createDeviceUsers).not.toHaveBeenCalled();
  expect(api.deleteDeviceUsers).not.toHaveBeenCalled();
});
