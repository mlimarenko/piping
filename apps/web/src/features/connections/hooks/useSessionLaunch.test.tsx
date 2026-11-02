import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { PropsWithChildren } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useSessionLaunch } from "./useSessionLaunch";
import type { ConnectionLaunchAccount, OpenSessionResponse } from "@/shared/api/types";

const mocks = vi.hoisted(() => ({ open: vi.fn(), close: vi.fn(), options: vi.fn(), save: vi.fn() }));
vi.mock("../api", () => ({ openSession: mocks.open, closeSession: mocks.close, getConnectionLaunchOptions: mocks.options }));
vi.mock("@/features/user-settings/api", () => ({ createMyCredential: mocks.save, setDefaultMyCredential: vi.fn() }));

const failed = { id: "old-a", hostId: "host-a", hostName: "SSH A", protocol: "ssh" as const };
const account: ConnectionLaunchAccount = { entryId: "current-account", kind: "shared", label: "Operator", username: "qa", requiresInput: true, isDefault: false, source: {} };
const opened = { session: { id: "new-a", hostId: "host-a", protocol: "ssh" }, proxy: { websocketUrl: "/ssh/new-a", token: "synthetic-token" } } as OpenSessionResponse;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function setup() {
  const accept = vi.fn((_response: OpenSessionResponse, _destination: string, _username?: string) => true);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: PropsWithChildren) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...renderHook(() => useSessionLaunch<string>({ accept }), { wrapper }), accept };
}

describe("fresh session launch controller", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [account] }] });
    mocks.open.mockResolvedValue(opened);
    mocks.close.mockResolvedValue(undefined);
  });

  it.each([[0, 1], [1, 0]])("accepts independent normal launches in response order %j then %j", async (first, second) => {
    const requests = [deferred<OpenSessionResponse>(), deferred<OpenSessionResponse>()];
    mocks.open.mockReturnValueOnce(requests[0]!.promise).mockReturnValueOnce(requests[1]!.promise);
    const { result, accept } = setup();
    const promises: Promise<boolean>[] = [];
    act(() => {
      promises.push(result.current.launch({ hostId: "host-a", protocol: "ssh" }, "normal-a"));
      promises.push(result.current.launch({ hostId: "host-b", protocol: "ssh" }, "normal-b"));
    });
    for (const index of [first, second]) await act(async () => {
      requests[index]!.resolve({ ...opened, session: { ...opened.session, id: `new-${index}` } });
      await promises[index];
    });
    expect(accept.mock.calls.map(call => call[1])).toEqual([`normal-${first === 0 ? "a" : "b"}`, `normal-${second === 0 ? "a" : "b"}`]);
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it.each(["credentials", "retry"])("does not cancel a normal POST or clear a new %s intent", async mode => {
    const request = deferred<OpenSessionResponse>();
    mocks.open.mockReturnValueOnce(request.promise);
    const { result, accept } = setup();
    let pending!: Promise<boolean>;
    act(() => { pending = result.current.launch({ hostId: "host-b", protocol: "ssh" }, "normal-b"); });
    await act(async () => {
      if (mode === "retry") await result.current.retry(failed, "replace-a");
      else result.current.openCredentials({ protocol: "ssh", host: { id: "host-a", name: "A" }, account: { entryId: account.entryId, name: account.label } }, "credential-a");
    });
    const interactive = result.current.intent?.attempt;
    await act(async () => { request.resolve(opened); await pending; });
    expect(accept).toHaveBeenCalledExactlyOnceWith(opened, "normal-b", undefined);
    expect(result.current.intent?.attempt).toBe(interactive);
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it("does not invalidate an in-flight replacement when a normal launch starts", async () => {
    const replacement = deferred<OpenSessionResponse>();
    mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ ...account, requiresInput: false }] }] });
    mocks.open.mockReturnValueOnce(replacement.promise).mockResolvedValueOnce({ ...opened, session: { ...opened.session, id: "new-b" } });
    const { result, accept } = setup();
    let pending!: Promise<void>;
    await act(async () => { pending = result.current.retry(failed, "replace-a"); await Promise.resolve(); });
    await act(async () => { await result.current.launch({ hostId: "host-b", protocol: "ssh" }, "normal-b"); });
    await act(async () => { replacement.resolve(opened); await pending; });
    expect(accept.mock.calls.map(call => call[1])).toEqual(["normal-b", "replace-a"]);
    expect(mocks.close).toHaveBeenCalledExactlyOnceWith("old-a");
  });

  it("ignores stale selection, submission and cancellation callbacks from another form attempt", async () => {
    const { result } = setup();
    const context = { protocol: "ssh" as const, host: { id: "host-a", name: "A" }, account: { entryId: account.entryId, name: account.label } };
    act(() => result.current.openCredentials(context, "form-a"));
    const oldId = result.current.intent!.attempt;
    act(() => result.current.openCredentials({ ...context, host: { id: "host-b", name: "B" } }, "form-b"));
    const bId = result.current.intent!.attempt;
    await act(async () => {
      await result.current.choose(account, oldId);
      await result.current.submit({ host: context.host, protocol: "ssh", entryId: account.entryId, credentials: { password: "synthetic" }, makeDefault: false }, oldId);
      result.current.cancel(oldId);
      await result.current.refresh(oldId);
    });
    expect(result.current.intent?.attempt).toBe(bId);
    expect(result.current.intent?.destination).toBe("form-b");
    expect(mocks.open).not.toHaveBeenCalled();
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it("asks for fresh credentials, ignores duplicate Retry, and closes old only after accepted replacement", async () => {
    const { result, accept } = setup();
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    expect(mocks.options).toHaveBeenCalledTimes(1);
    expect(result.current.intent?.context?.account.entryId).toBe(account.entryId);
    expect(mocks.open).not.toHaveBeenCalled();
    expect(mocks.close).not.toHaveBeenCalled();
    const password = crypto.randomUUID();
    await act(async () => {
      await result.current.submit({ host: { id: failed.hostId, name: failed.hostName }, protocol: "ssh", entryId: account.entryId, username: "qa", credentials: { password }, makeDefault: false }, result.current.intent!.attempt);
    });
    const request = mocks.open.mock.calls[0]?.[0] as { entryId?: string; credentials?: { password?: string } };
    expect(request.entryId).toBe(account.entryId);
    expect(request.credentials?.password === password).toBe(true);
    expect(accept).toHaveBeenCalledWith(opened, "replace-a", "qa");
    expect(mocks.close).toHaveBeenCalledExactlyOnceWith("old-a");
    expect(mocks.save).not.toHaveBeenCalled();
    expect(result.current.intent).toBeNull();
  });

  it("cancel does not close or open anything and re-entry resolves current launch accounts", async () => {
    const { result } = setup();
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    act(() => result.current.cancel(result.current.intent!.attempt));
    expect(mocks.open).not.toHaveBeenCalled();
    expect(mocks.close).not.toHaveBeenCalled();
    mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ ...account, entryId: "changed", requiresInput: false }] }] });
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    expect(mocks.open.mock.calls[0]?.[0]).toMatchObject({ entryId: "changed" });
  });

  it("requires explicit selection for multiple presentable accounts", async () => {
    mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [account, { ...account, entryId: "second", username: "other" }] }] });
    const { result } = setup();
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    expect(result.current.intent?.accounts).toHaveLength(2);
    expect(result.current.intent?.context).toBeUndefined();
    expect(mocks.open).not.toHaveBeenCalled();
    await act(async () => { await result.current.choose(account, result.current.intent!.attempt); });
    expect(result.current.intent?.context?.account.entryId).toBe(account.entryId);
  });

  it.each(["empty", "rejected"])("does not invent a bare launch when current options are %s", async mode => {
    if (mode === "empty") mocks.options.mockResolvedValue({ protocols: [] });
    else mocks.options.mockRejectedValue(new Error("private backend detail"));
    const { result } = setup();
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    expect(Boolean(result.current.intent?.error)).toBe(true);
    expect(result.current.intent?.error?.includes("private backend detail")).toBe(false);
    expect(mocks.open).not.toHaveBeenCalled();
    expect(mocks.close).not.toHaveBeenCalled();
  });

  it("discards stale option responses", async () => {
    let resolve!: (value: unknown) => void;
    mocks.options.mockReturnValueOnce(new Promise(done => { resolve = done; }));
    const { result } = setup();
    let pending!: Promise<void>;
    act(() => { pending = result.current.retry(failed, "replace-a"); });
    act(() => result.current.cancel(result.current.intent!.attempt));
    await act(async () => { resolve({ protocols: [{ protocol: "ssh", accounts: [{ ...account, requiresInput: false }] }] }); await pending; });
    expect(mocks.open).not.toHaveBeenCalled();
    expect(result.current.intent).toBeNull();
  });

  it.each(["cancel", "closed-target"])("closes only the new orphan when a POST completes after %s", async mode => {
    let resolve!: (value: OpenSessionResponse) => void;
    mocks.open.mockReturnValueOnce(new Promise(done => { resolve = done; }));
    mocks.options.mockResolvedValue({ protocols: [{ protocol: "ssh", accounts: [{ ...account, requiresInput: false }] }] });
    const { result, accept } = setup();
    let pending!: Promise<void>;
    await act(async () => { pending = result.current.retry(failed, "replace-a"); await Promise.resolve(); });
    if (mode === "cancel") act(() => result.current.cancel(result.current.intent!.attempt));
    else accept.mockReturnValue(false);
    await act(async () => { resolve(opened); await pending; });
    expect(mocks.close).toHaveBeenCalledExactlyOnceWith("new-a");
  });

  it("keeps failed submission retryable without closing the original", async () => {
    mocks.open.mockRejectedValueOnce(new Error("failed"));
    const { result } = setup();
    await act(async () => { await result.current.retry(failed, "replace-a"); });
    await act(async () => {
      await expect(result.current.submit({ host: { id: failed.hostId, name: failed.hostName }, protocol: "ssh", entryId: account.entryId, credentials: { password: "synthetic" }, makeDefault: false }, result.current.intent!.attempt)).rejects.toThrow("failed");
    });
    expect(result.current.intent?.submitting).not.toBe(true);
    expect(Boolean(result.current.intent?.context)).toBe(true);
    expect(mocks.close).not.toHaveBeenCalled();
    expect(JSON.stringify(result.current.intent).includes("synthetic")).toBe(false);
  });
});
