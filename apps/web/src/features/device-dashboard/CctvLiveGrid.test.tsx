import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import CctvLiveGrid from "@/features/device-dashboard/CctvLiveGrid";

const liveViewMock = vi.hoisted(() => vi.fn());
const snapshotViewMock = vi.hoisted(() => vi.fn());

vi.mock("@/features/connections/components/connections/DeviceLiveView", () => ({
  default: (props: Record<string, unknown>) => {
    liveViewMock(props);
    return <div data-testid="live-view">{props.overlay as React.ReactNode}</div>;
  },
}));
vi.mock("@/features/connections/components/connections/DeviceSnapshotView", () => ({
  default: (props: Record<string, unknown>) => {
    snapshotViewMock(props);
    return <div data-testid="snapshot-view" />;
  },
}));

const channels = [
  { index: 1, name: " Entrance ", enabled: true, ptz_support: "supported" as const, recording: true },
  { index: 2, name: null, enabled: true, ptz_support: "unknown" as const, recording: false },
];

describe("CctvLiveGrid", () => {
  beforeEach(() => {
    liveViewMock.mockClear();
    snapshotViewMock.mockClear();
  });

  it("renders stream cells with named channel pickers and forwards user actions", () => {
    const onCellChannelChange = vi.fn();
    const onFocusChannel = vi.fn();
    render(
      <CctvLiveGrid
        targetId="target-1"
        layout={2}
        cells={[1, 2]}
        source="stream"
        channels={channels}
        onCellChannelChange={onCellChannelChange}
        onFocusChannel={onFocusChannel}
        active={false}
      />,
    );

    expect(screen.getAllByTestId("live-view")).toHaveLength(2);
    expect(snapshotViewMock).not.toHaveBeenCalled();
    expect(liveViewMock).toHaveBeenCalledWith(
      expect.objectContaining({ targetId: "target-1", channel: 1, fps: 3, maxWidth: 640, active: false }),
    );
    expect(screen.getAllByRole("option", { name: "1 · Entrance" })).toHaveLength(2);
    expect(screen.getAllByRole("option", { name: "Ch 2" })).toHaveLength(2);
    const pickers = screen.getAllByRole("combobox", { name: "Cell channel" });
    expect(pickers[0]).toHaveValue("1");
    expect(pickers[1]).toHaveValue("2");
    fireEvent.click(pickers[0]!);
    expect(onFocusChannel).not.toHaveBeenCalled();

    fireEvent.change(screen.getAllByRole("combobox", { name: "Cell channel" })[0]!, {
      target: { value: "2" },
    });
    expect(onCellChannelChange).toHaveBeenCalledWith(0, 2);

    fireEvent.click(screen.getAllByRole("button", { name: "Open this channel full-size" })[1]!);
    expect(onFocusChannel).toHaveBeenCalledWith(2);
  });

  it("renders paused still cells and derives picker options from cells when discovery is unavailable", () => {
    render(
      <CctvLiveGrid
        targetId="target-2"
        layout={3}
        cells={[4, 7]}
        source="stills"
        channels={[]}
        onCellChannelChange={vi.fn()}
        onFocusChannel={vi.fn()}
      />,
    );

    expect(screen.getAllByTestId("snapshot-view")).toHaveLength(2);
    expect(liveViewMock).not.toHaveBeenCalled();
    expect(snapshotViewMock).toHaveBeenCalledWith(
      expect.objectContaining({
        targetId: "target-2",
        channel: 4,
        intervalMs: 5000,
        autoRefreshDefault: true,
        hideControls: true,
        active: true,
      }),
    );
    expect(screen.getAllByRole("option", { name: "Ch 4" })).toHaveLength(2);
    expect(screen.getAllByRole("option", { name: "Ch 7" })).toHaveLength(2);
  });
});
