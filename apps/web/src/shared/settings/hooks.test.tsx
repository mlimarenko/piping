import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useSetting } from "./hooks";
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

  function Harness() {
    const setting = useSetting(TEST_SETTING_KEY, "fallback", {
      bootstrapCache: options?.bootstrapCache,
      debounceMs: options?.debounceMs,
      enabled: false,
    });
    return (
      <>
        <button type="button" onClick={() => setting.setValue("custom")}>
          set
        </button>
        <button type="button" onClick={setting.resetValue}>
          reset
        </button>
      </>
    );
  }

  const view = render(
    <QueryClientProvider client={client}>
      <Harness />
    </QueryClientProvider>,
  );
  return { client, ...view };
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
});
