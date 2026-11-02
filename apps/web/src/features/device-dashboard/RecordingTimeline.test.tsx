import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import RecordingTimeline from "./RecordingTimeline";
import type * as DeviceApi from "./api";

const apiMock = vi.hoisted(() => ({ searchDeviceRecordings: vi.fn() }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof DeviceApi>();
  return { ...actual, searchDeviceRecordings: apiMock.searchDeviceRecordings };
});

beforeEach(() => {
  apiMock.searchDeviceRecordings.mockReset();
});

function renderTimeline(overrides: Partial<React.ComponentProps<typeof RecordingTimeline>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onSelectSpan = vi.fn();
  const result = render(
    <QueryClientProvider client={client}>
      <RecordingTimeline
        targetId="target-1"
        channel={1}
        selected={null}
        onSelectSpan={onSelectSpan}
        {...overrides}
      />
    </QueryClientProvider>,
  );
  return { ...result, onSelectSpan };
}

describe("RecordingTimeline", () => {
  it("queries the per-channel search endpoint for the current day", async () => {
    apiMock.searchDeviceRecordings.mockResolvedValue({ spans: [] });
    renderTimeline({ channel: 3 });

    await waitFor(() => expect(apiMock.searchDeviceRecordings).toHaveBeenCalled());
    const [targetId, window] = apiMock.searchDeviceRecordings.mock.calls[0] as [
      string,
      { channel: number; startIso: string; endIso: string },
    ];
    expect(targetId).toBe("target-1");
    expect(window.channel).toBe(3);
    expect(Date.parse(window.endIso) - Date.parse(window.startIso)).toBe(24 * 60 * 60 * 1000);
  });

  it("renders a clickable segment for each returned span", async () => {
    const now = new Date();
    const dayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const spanStart = new Date(dayStart.getTime() + 6 * 60 * 60 * 1000);
    const spanEnd = new Date(dayStart.getTime() + 7 * 60 * 60 * 1000);
    apiMock.searchDeviceRecordings.mockResolvedValue({
      spans: [{ start: spanStart.toISOString(), end: spanEnd.toISOString() }],
    });

    const { onSelectSpan } = renderTimeline();

    const segment = await screen.findByTitle(
      `${spanStart.toLocaleTimeString()} → ${spanEnd.toLocaleTimeString()}`,
    );
    fireEvent.click(segment);

    expect(onSelectSpan).toHaveBeenCalledTimes(1);
    const window = onSelectSpan.mock.calls[0]![0] as { startMs: number; endMs: number };
    expect(window.startMs).toBe(spanStart.getTime());
    expect(window.endMs).toBe(spanEnd.getTime());
  });

  it("shows a no-footage message when the day has no spans", async () => {
    apiMock.searchDeviceRecordings.mockResolvedValue({ spans: [] });
    renderTimeline();

    expect(await screen.findByText(/no recorded footage this day/i)).toBeInTheDocument();
  });

  it("shows an unavailable message instead of crashing when the search fails", async () => {
    apiMock.searchDeviceRecordings.mockRejectedValue(new Error("device offline"));
    renderTimeline();

    expect(await screen.findByText(/timeline unavailable/i)).toBeInTheDocument();
  });

  it("disables Next day while viewing today", async () => {
    apiMock.searchDeviceRecordings.mockResolvedValue({ spans: [] });
    renderTimeline();

    await waitFor(() => expect(apiMock.searchDeviceRecordings).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: /next day/i })).toBeDisabled();
  });

  it("re-queries the previous day and re-enables Next day when navigating back", async () => {
    apiMock.searchDeviceRecordings.mockResolvedValue({ spans: [] });
    renderTimeline();

    await waitFor(() => expect(apiMock.searchDeviceRecordings).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole("button", { name: /prev day/i }));

    await waitFor(() => expect(apiMock.searchDeviceRecordings).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("button", { name: /next day/i })).not.toBeDisabled();
  });
});
