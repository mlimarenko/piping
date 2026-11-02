import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  deleteUserSetting,
  upsertUserSetting,
  type EffectiveSettingDto,
} from "@/shared/settings/api";
import { useTableColumnVisibility } from "./useTableColumnVisibility";

vi.mock("@/shared/settings/api", () => ({
  deleteAdminSettingDefault: vi.fn(),
  deleteUserSetting: vi.fn(),
  fetchAdminSettingDefaults: vi.fn(),
  fetchEffectiveSetting: vi.fn(),
  fetchEffectiveSettings: vi.fn(),
  upsertAdminSettingDefault: vi.fn(),
  upsertUserSetting: vi.fn(),
}));

const SETTING_KEY = "table.column_visibility";
const QUERY_KEY = ["settings", "effective", SETTING_KEY] as const;

function renderHookHarness(initialValue: unknown, tableId = "alpha") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData<EffectiveSettingDto>(QUERY_KEY, {
    key: SETTING_KEY,
    value: initialValue,
    source: "user",
    user_value: initialValue,
    instance_value: undefined,
    updated_at: "2026-06-05T00:00:00.000Z",
  });

  function Harness() {
    const { hiddenColumns, setHidden, reset } = useTableColumnVisibility(tableId);
    return (
      <>
        <span data-testid="hidden">{Array.from(hiddenColumns).sort().join(",")}</span>
        <button type="button" onClick={() => setHidden(new Set(["status"]))}>
          hide
        </button>
        <button type="button" onClick={reset}>
          reset
        </button>
      </>
    );
  }

  return {
    client,
    ...render(
      <QueryClientProvider client={client}>
        <Harness />
      </QueryClientProvider>,
    ),
  };
}

async function flushMicrotasks() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("useTableColumnVisibility", () => {
  const upsertMock = vi.mocked(upsertUserSetting);
  const deleteMock = vi.mocked(deleteUserSetting);

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("reads only the requested table's hidden columns", () => {
    renderHookHarness({ alpha: { hidden: ["status"] }, beta: { hidden: ["owner"] } });
    expect(screen.getByTestId("hidden").textContent).toBe("status");
  });

  it("normalizes stored hidden keys before exposing them to the hook", () => {
    renderHookHarness({
      " alpha ": {
        hidden: ["status", " status ", "", "owner", 17, "owner"],
      },
      beta: { hidden: ["ghost"] },
    });
    expect(screen.getByTestId("hidden").textContent).toBe("owner,status");
  });

  it("uses default-hidden columns when the table has no stored override", () => {
    renderHookHarness({}, "inventory.Devices");
    expect(screen.getByTestId("hidden").textContent).toBe(
      "airflow,asset_tag,face,position,primary_ip,serial",
    );
  });

  it("treats an explicit empty hidden list as a user override over defaults", () => {
    renderHookHarness({ "inventory.Devices": { hidden: [] } }, "inventory.Devices");
    expect(screen.getByTestId("hidden").textContent).toBe("");
  });

  it("setHidden persists this table without clobbering sibling tables", async () => {
    vi.useFakeTimers();
    upsertMock.mockResolvedValue(undefined);
    renderHookHarness({ beta: { hidden: ["owner"] } });

    fireEvent.click(screen.getByRole("button", { name: "hide" }));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    await flushMicrotasks();

    expect(upsertMock).toHaveBeenCalledTimes(1);
    expect(upsertMock).toHaveBeenCalledWith(SETTING_KEY, {
      beta: { hidden: ["owner"] },
      alpha: { hidden: ["status"] },
    });
  });

  it("reads a previously persisted hidden-column override after remount", async () => {
    vi.useFakeTimers();
    upsertMock.mockResolvedValue(undefined);
    const firstMount = renderHookHarness({ beta: { hidden: ["owner"] } });

    fireEvent.click(screen.getByRole("button", { name: "hide" }));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    await flushMicrotasks();

    const persistedValue = upsertMock.mock.calls[0]?.[1];
    expect(persistedValue).toEqual({
      beta: { hidden: ["owner"] },
      alpha: { hidden: ["status"] },
    });

    firstMount.unmount();
    renderHookHarness(persistedValue);

    expect(screen.getByTestId("hidden").textContent).toBe("status");
  });

  it("reset preserves sibling tables and writes the trimmed record", async () => {
    vi.useFakeTimers();
    upsertMock.mockResolvedValue(undefined);
    renderHookHarness({ alpha: { hidden: ["status"] }, beta: { hidden: ["owner"] } });

    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    await flushMicrotasks();

    expect(deleteMock).not.toHaveBeenCalled();
    expect(upsertMock).toHaveBeenCalledWith(SETTING_KEY, { beta: { hidden: ["owner"] } });
  });

  it("reset drops the whole user override when no other table remains", async () => {
    deleteMock.mockResolvedValue(undefined);
    renderHookHarness({ alpha: { hidden: ["status"] } });

    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    await flushMicrotasks();

    expect(deleteMock).toHaveBeenCalledWith(SETTING_KEY);
    expect(upsertMock).not.toHaveBeenCalled();
  });

  it("is inert when tableId is empty", async () => {
    vi.useFakeTimers();
    renderHookHarness({ alpha: { hidden: ["status"] } }, "");

    expect(screen.getByTestId("hidden").textContent).toBe("");

    fireEvent.click(screen.getByRole("button", { name: "hide" }));
    fireEvent.click(screen.getByRole("button", { name: "reset" }));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    await flushMicrotasks();

    expect(upsertMock).not.toHaveBeenCalled();
    expect(deleteMock).not.toHaveBeenCalled();
  });
});
