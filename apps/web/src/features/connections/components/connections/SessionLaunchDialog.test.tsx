import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { useSessionLaunch } from "../../hooks/useSessionLaunch";
import SessionLaunchDialog from "./SessionLaunchDialog";
import type { OpenSessionResponse } from "@/shared/api/types";

const mocks = vi.hoisted(() => ({ open: vi.fn(), close: vi.fn(), save: vi.fn(), accept: vi.fn(), notify: vi.fn() }));
vi.mock("../../api", () => ({ openSession: mocks.open, closeSession: mocks.close, getConnectionLaunchOptions: vi.fn() }));
vi.mock("@/features/user-settings/api", () => ({ createMyCredential: mocks.save, setDefaultMyCredential: vi.fn() }));
vi.mock("@/features/auth/api", () => ({ me: vi.fn(() => Promise.resolve({ username: "qa" })) }));
vi.mock("@/shared/lib/notifications", () => ({ notify: mocks.notify }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const response = (id: string) => ({ session: { id, hostId: id, protocol: "ssh" }, proxy: { websocketUrl: `/ssh/${id}`, token: "fixture" } }) as OpenSessionResponse;
function Harness() {
  const controller = useSessionLaunch<string>({ accept: mocks.accept });
  return <>
    {["A", "B"].map(id => <button key={id} onClick={() => controller.openCredentials({ protocol: "ssh", host: { id, name: id }, account: { entryId: id, name: id, username: "qa" } }, id)}>Open {id}</button>)}
    <SessionLaunchDialog controller={controller} />
  </>;
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.accept.mockReturnValue(true);
  mocks.close.mockResolvedValue(undefined);
});

it.each([{ submitted: false, saveFails: false }, { submitted: true, saveFails: false }, { submitted: false, saveFails: true }])("late optional save A cannot cancel B (submitted=$submitted, saveFails=$saveFails)", async ({ submitted, saveFails }) => {
  const save = deferred<{ id: string }>();
  const bRequest = deferred<OpenSessionResponse>();
  mocks.save.mockReturnValueOnce(save.promise);
  mocks.open.mockResolvedValueOnce(response("new-a")).mockReturnValueOnce(bRequest.promise);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Open A" }));
  const a = await screen.findByRole("dialog");
  fireEvent.change(within(a).getByLabelText(/Password/), { target: { value: "synthetic-a" } });
  fireEvent.click(within(a).getByRole("checkbox"));
  fireEvent.click(within(a).getByRole("button", { name: "Connect" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  expect(mocks.save).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "Open B" }));
  const b = await screen.findByRole("dialog");
  const input = within(b).getByLabelText(/Password/);
  fireEvent.change(input, { target: { value: "synthetic-b" } });
  if (submitted) {
    fireEvent.click(within(b).getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(mocks.open).toHaveBeenCalledTimes(2));
  }
  await act(async () => {
    if (saveFails) save.reject(new Error("synthetic save failure"));
    else save.resolve({ id: "saved-a" });
    await save.promise.catch(() => undefined);
  });
  if (saveFails) expect(mocks.notify).toHaveBeenCalledWith("error", expect.any(String));
  expect(screen.getByRole("dialog")).toBe(b);
  expect(input === within(b).getByLabelText(/Password/)).toBe(true);
  expect((input as HTMLInputElement).value === "synthetic-b").toBe(true);
  expect(mocks.close).not.toHaveBeenCalled();
  if (!submitted) fireEvent.click(within(b).getByRole("button", { name: "Connect" }));
  await act(async () => { bRequest.resolve(response("new-b")); await bRequest.promise; });
  await waitFor(() => expect(mocks.accept).toHaveBeenCalledTimes(2));
  expect(mocks.accept.mock.calls.map(call => call[1])).toEqual(["A", "B"]);
  expect(mocks.close).not.toHaveBeenCalled();
});
