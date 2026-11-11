import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useSetting } from "./hooks";
import { writeCachedSettingValue } from "./local-cache";
import { deleteUserSetting, upsertUserSetting, type EffectiveSettingDto } from "./api";

vi.mock("./api", () => ({
  deleteAdminSettingDefault: vi.fn(),
  deleteUserSetting: vi.fn(),
  fetchAdminSettingDefaults: vi.fn(),
  fetchEffectiveSetting: vi.fn(),
  fetchEffectiveSettings: vi.fn(),
  upsertAdminSettingDefault: vi.fn(),
  upsertUserSetting: vi.fn(),
}));

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve;
    reject = innerReject;
  });
  return { promise, reject, resolve };
}

const TEST_SETTING_KEY = "test.setting";
const TEST_SETTING_QUERY_KEY = ["settings", "effective", TEST_SETTING_KEY] as const;
const TEST_SETTING_CACHE_KEY = `piping.settings.cache.v1:${TEST_SETTING_KEY}`;

function renderSettingHarness(options?: {
  bootstrapCache?: boolean;
  debounceMs?: number;
  initialSetting?: EffectiveSettingDto;
}) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });
  if (options?.initialSetting) {
    client.setQueryData(TEST_SETTING_QUERY_KEY, options.initialSetting);
  }
  const writes: Promise<boolean>[] = [];

  function Harness() {
    const setting = useSetting(TEST_SETTING_KEY, "fallback", {
      bootstrapCache: options?.bootstrapCache,
      debounceMs: options?.debounceMs,
      enabled: false,
    });
    return (
      <>
        <button type="button" onClick={() => { writes.push(setting.setValue("custom")); }}>
          set
        </button>
        <button type="button" onClick={() => { writes.push(setting.setValue("newer")); }}>
          set newer
        </button>
        <button type="button" onClick={() => { writes.push(setting.resetValue()); }}>
          reset
        </button>
        <span data-testid="setting-value">{String(setting.value)}</span>
      </>
    );
  }

  const view = render(
    <QueryClientProvider client={client}>
      <Harness />
    </QueryClientProvider>,
  );
  return { client, writes, ...view };
}

async function flushMicrotasks() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("useSetting", () => {
  const upsertUserSettingMock = vi.mocked(upsertUserSetting);
  const deleteUserSettingMock = vi.mocked(deleteUserSetting);

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    window.localStorage.clear();
  });

  it.each([
    undefined,
    { key: TEST_SETTING_KEY, source: "unset" as const },
    { key: TEST_SETTING_KEY, source: "unset" as const, value: null },
  ])(
    "does not persist a fallback as a user preference when the setting is %j",
    (initialSetting) => {
      renderSettingHarness({ bootstrapCache: true, initialSetting });
      expect(window.localStorage.getItem(TEST_SETTING_CACHE_KEY)).toBeNull();
    },
  );

  it("serializes reset after an in-flight user setting write", async () => {
    const write = deferred<void>();
    upsertUserSettingMock.mockReturnValue(write.promise);
    deleteUserSettingMock.mockResolvedValue(undefined);
    renderSettingHarness();

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    await flushMicrotasks();
    expect(upsertUserSettingMock).toHaveBeenCalledWith(TEST_SETTING_KEY, "custom");

    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    await flushMicrotasks();
    expect(deleteUserSettingMock).not.toHaveBeenCalled();

    write.resolve();
    await flushMicrotasks();

    expect(deleteUserSettingMock).toHaveBeenCalledWith(TEST_SETTING_KEY);
    expect(upsertUserSettingMock.mock.invocationCallOrder[0]!).toBeLessThan(
      deleteUserSettingMock.mock.invocationCallOrder[0]!,
    );
  });

  it("cancels a pending debounced write before reset", async () => {
    vi.useFakeTimers();
    upsertUserSettingMock.mockResolvedValue(undefined);
    deleteUserSettingMock.mockResolvedValue(undefined);
    renderSettingHarness({ debounceMs: 100 });

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    await flushMicrotasks();

    expect(deleteUserSettingMock).toHaveBeenCalledWith(TEST_SETTING_KEY);

    act(() => {
      vi.advanceTimersByTime(100);
    });

    expect(upsertUserSettingMock).not.toHaveBeenCalled();
  });

  it("keeps bootstrap cache on the reset target after a stale refetch", async () => {
    const write = deferred<void>();
    const reset = deferred<void>();
    upsertUserSettingMock.mockReturnValue(write.promise);
    deleteUserSettingMock.mockReturnValue(reset.promise);
    const { client } = renderSettingHarness({
      bootstrapCache: true,
      initialSetting: {
        key: TEST_SETTING_KEY,
        value: "instance",
        source: "instance",
        instance_value: "instance",
        user_value: undefined,
        updated_at: "2026-06-05T00:00:00.000Z",
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    await flushMicrotasks();
    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    await flushMicrotasks();

    write.resolve();
    await flushMicrotasks();
    expect(deleteUserSettingMock).toHaveBeenCalledWith(TEST_SETTING_KEY);

    act(() => {
      client.setQueryData<EffectiveSettingDto>(TEST_SETTING_QUERY_KEY, {
        key: TEST_SETTING_KEY,
        value: "custom",
        source: "user",
        instance_value: "instance",
        user_value: "custom",
        updated_at: "2026-06-05T00:00:01.000Z",
      });
    });
    // The stale refetch lands via the query cache, but the component re-render
    // (and the value -> bootstrap-cache effect that writes "custom") is dispatched
    // on a setTimeout(0) macrotask by TanStack Query's notifyManager. Await that
    // propagation instead of reading the cache synchronously, which races the timer.
    await waitFor(() => {
      const staleCache = JSON.parse(
        window.localStorage.getItem(TEST_SETTING_CACHE_KEY) ?? "{}",
      ) as {
        value?: unknown;
      };
      expect(staleCache.value).toBe("custom");
    });

    reset.resolve();
    await flushMicrotasks();
    await flushMicrotasks();

    const finalCache = JSON.parse(window.localStorage.getItem(TEST_SETTING_CACHE_KEY) ?? "{}") as {
      value?: unknown;
    };
    expect(finalCache.value).toBe("instance");
  });

  it("keeps the optimistic value and resolves true only after the server accepts the write", async () => {
    const write = deferred<void>();
    upsertUserSettingMock.mockReturnValue(write.promise);
    const { writes } = renderSettingHarness({
      initialSetting: {
        key: TEST_SETTING_KEY,
        value: "instance",
        source: "instance",
        instance_value: "instance",
        user_value: undefined,
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    await waitFor(() => {
      expect(screen.getByTestId("setting-value")).toHaveTextContent("custom");
    });
    expect(upsertUserSettingMock).toHaveBeenCalledWith(TEST_SETTING_KEY, "custom");

    let settled = false;
    void writes[0]!.then(() => {
      settled = true;
    });
    await flushMicrotasks();
    expect(settled).toBe(false);

    write.resolve();
    await expect(writes[0]).resolves.toBe(true);
  });

  it("resolves false and restores the effective value when the save is rejected", async () => {
    upsertUserSettingMock.mockRejectedValue(new Error("write rejected"));
    const { writes } = renderSettingHarness({
      initialSetting: {
        key: TEST_SETTING_KEY,
        value: "instance",
        source: "instance",
        instance_value: "instance",
        user_value: undefined,
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    await expect(writes[0]).resolves.toBe(false);
    await waitFor(() => {
      expect(screen.getByTestId("setting-value")).toHaveTextContent("instance");
    });
  });

  it("resolves false and restores the user override when the reset is rejected", async () => {
    deleteUserSettingMock.mockRejectedValue(new Error("delete rejected"));
    const { writes } = renderSettingHarness({
      initialSetting: {
        key: TEST_SETTING_KEY,
        value: "custom",
        source: "user",
        instance_value: "instance",
        user_value: "custom",
      },
    });

    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    await expect(writes[0]).resolves.toBe(false);
    await waitFor(() => {
      expect(screen.getByTestId("setting-value")).toHaveTextContent("custom");
    });
  });

  it("resolves a superseded save as false and the latest save as true", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    upsertUserSettingMock
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const { writes } = renderSettingHarness();

    fireEvent.click(screen.getByRole("button", { name: "set" }));
    await flushMicrotasks();
    fireEvent.click(screen.getByRole("button", { name: "set newer" }));
    await flushMicrotasks();

    first.resolve();
    await expect(writes[0]).resolves.toBe(false);
    await flushMicrotasks();

    second.resolve();
    await expect(writes[1]).resolves.toBe(true);
  });
});

describe("useSetting normalized value identity", () => {
  const OBJECT_SETTING_KEY = "test.object.setting";
  const OBJECT_SETTING_QUERY_KEY = ["settings", "effective", OBJECT_SETTING_KEY] as const;

  interface KeysValue {
    keys: string[];
  }

  // Product normalizers such as the tree-collapse one rebuild their result from
  // the raw input on every call, so the same raw value yields a new object.
  function normalizeKeys(value: unknown): KeysValue {
    const raw = (value as { keys?: unknown } | null)?.keys;
    return {
      keys: Array.isArray(raw) ? raw.filter((key): key is string => typeof key === "string") : [],
    };
  }

  function normalizeUpperKeys(value: unknown): KeysValue {
    return { keys: normalizeKeys(value).keys.map((key) => key.toUpperCase()) };
  }

  const FALLBACK_A: KeysValue = { keys: ["fallback-a"] };
  const FALLBACK_B: KeysValue = { keys: ["fallback-b"] };

  /**
   * Mirrors a consumer that keeps its own state (the connections tree does this
   * on scroll) and reacts to the setting through an effect, the way the tree
   * hydrates collapsed groups. `seen` collects every value the effect observed.
   */
  function IdentityHarness(props: {
    fallback: KeysValue;
    normalize: (value: unknown) => KeysValue;
    bootstrapCache?: boolean;
    seen: KeysValue[];
    seenSetters: unknown[];
  }) {
    const [tick, setTick] = useState(0);
    const setting = useSetting<KeysValue>(OBJECT_SETTING_KEY, props.fallback, {
      bootstrapCache: props.bootstrapCache,
      enabled: false,
      normalize: props.normalize,
    });
    useEffect(() => {
      props.seen.push(setting.value);
    }, [props.seen, setting.value]);
    useEffect(() => {
      props.seenSetters.push(setting.setValue, setting.resetValue);
    }, [props.seenSetters, setting.setValue, setting.resetValue]);
    return (
      <button type="button" onClick={() => setTick((count) => count + 1)}>
        rerender {tick}
      </button>
    );
  }

  function renderIdentityHarness(options: {
    fallback: KeysValue;
    normalize: (value: unknown) => KeysValue;
    bootstrapCache?: boolean;
    initialValue?: unknown;
  }) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    if (options.initialValue !== undefined) {
      client.setQueryData<EffectiveSettingDto>(OBJECT_SETTING_QUERY_KEY, {
        key: OBJECT_SETTING_KEY,
        value: options.initialValue,
        source: "user",
        user_value: options.initialValue,
      });
    }
    const seen: KeysValue[] = [];
    const seenSetters: unknown[] = [];
    const element = (next?: {
      fallback?: KeysValue;
      normalize?: (value: unknown) => KeysValue;
    }) => (
      <QueryClientProvider client={client}>
        <IdentityHarness
          fallback={next?.fallback ?? options.fallback}
          normalize={next?.normalize ?? options.normalize}
          bootstrapCache={options.bootstrapCache}
          seen={seen}
          seenSetters={seenSetters}
        />
      </QueryClientProvider>
    );
    const view = render(element());
    return {
      client,
      seen,
      seenSetters,
      rerender: (next?: { fallback?: KeysValue; normalize?: (value: unknown) => KeysValue }) =>
        view.rerender(element(next)),
    };
  }

  function updateServerValue(client: QueryClient, value: unknown) {
    act(() => {
      client.setQueryData<EffectiveSettingDto>(OBJECT_SETTING_QUERY_KEY, {
        key: OBJECT_SETTING_KEY,
        value,
        source: "user",
        user_value: value,
      });
    });
  }

  afterEach(() => {
    window.localStorage.clear();
  });

  it("keeps the value reference stable across rerenders that leave the setting unchanged", async () => {
    const { client, seen } = renderIdentityHarness({
      fallback: FALLBACK_A,
      normalize: normalizeKeys,
      initialValue: { keys: ["alpha"] },
    });
    expect(seen).toHaveLength(1);
    expect(seen[0]).toEqual({ keys: ["alpha"] });

    fireEvent.click(screen.getByRole("button"));
    fireEvent.click(screen.getByRole("button"));
    fireEvent.click(screen.getByRole("button"));
    expect(seen).toHaveLength(1);

    updateServerValue(client, { keys: ["beta"] });
    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ keys: ["beta"] });
  });

  it("does not restart write-callback consumers after unrelated rerenders", () => {
    const { seenSetters } = renderIdentityHarness({
      fallback: FALLBACK_A,
      normalize: normalizeKeys,
    });
    expect(seenSetters).toHaveLength(2);
    fireEvent.click(screen.getByRole("button"));
    fireEvent.click(screen.getByRole("button"));
    expect(seenSetters).toHaveLength(2);
  });

  it("keeps the cached bootstrap value stable and follows the server value afterwards", async () => {
    writeCachedSettingValue(OBJECT_SETTING_KEY, { keys: ["cached"] });
    const { client, seen } = renderIdentityHarness({
      bootstrapCache: true,
      fallback: FALLBACK_A,
      normalize: normalizeKeys,
    });
    expect(seen).toHaveLength(1);
    expect(seen[0]).toEqual({ keys: ["cached"] });

    fireEvent.click(screen.getByRole("button"));
    fireEvent.click(screen.getByRole("button"));
    expect(seen).toHaveLength(1);

    updateServerValue(client, { keys: ["server"] });
    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]).toEqual({ keys: ["server"] });
  });

  it("recomputes when the fallback or the normalizer input changes", () => {
    const { seen, rerender } = renderIdentityHarness({
      fallback: FALLBACK_A,
      normalize: normalizeKeys,
    });
    expect(seen).toEqual([{ keys: ["fallback-a"] }]);

    rerender({ fallback: FALLBACK_B });
    expect(seen).toEqual([{ keys: ["fallback-a"] }, { keys: ["fallback-b"] }]);

    rerender({ fallback: FALLBACK_B, normalize: normalizeUpperKeys });
    expect(seen.at(-1)).toEqual({ keys: ["FALLBACK-B"] });
    expect(seen).toHaveLength(3);
  });
});
