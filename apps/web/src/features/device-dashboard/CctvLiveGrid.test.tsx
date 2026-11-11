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

    fireEvent.click(screen.getByRole("button", { name: "Open this channel full-size: Ch 2" }));
    expect(onFocusChannel).toHaveBeenCalledWith(2);
  });

  it("does not start stale cells when discovery is unavailable", () => {
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

    expect(screen.queryByTestId("snapshot-view")).not.toBeInTheDocument();
    expect(liveViewMock).not.toHaveBeenCalled();
    expect(snapshotViewMock).not.toHaveBeenCalled();
    expect(screen.getAllByRole("option", { name: "Empty" })).toHaveLength(2);
  });

  it.each(["stream", "stills"] as const)("leaves stale and empty cells idle in %s mode", (source) => {
    render(<CctvLiveGrid targetId="target" layout={3} cells={[1, 2, 3, 4, 5, 0, 0, 0, 0]}
      source={source} channels={channels} onCellChannelChange={vi.fn()} onFocusChannel={vi.fn()} />);
    expect(source === "stream" ? liveViewMock : snapshotViewMock).toHaveBeenCalledTimes(2);
    expect(screen.getAllByRole("combobox", { name: "Cell channel" }).filter((el) => (el as HTMLSelectElement).value === "0")).toHaveLength(7);
    expect(screen.getByRole("button", { name: "Open this channel full-size: 1 · Entrance" })).toBeInTheDocument();
  });

  it("opens only discovered channels in a persisted 3×3 wall", () => {
    const fourChannels = [1, 2, 3, 4].map((index) => ({ ...channels[0]!, index, name: `Camera ${index}` }));
    render(<CctvLiveGrid targetId="target" layout={3} cells={[4, 2, 3, 1, 5, 6, 7, 8, 9]}
      source="stream" channels={fourChannels} onCellChannelChange={vi.fn()} onFocusChannel={vi.fn()} />);
    expect(liveViewMock.mock.calls.map(([props]) => props.channel)).toEqual([4, 2, 3, 1]);
    expect(screen.getAllByRole("combobox").slice(4).every((el) => (el as HTMLSelectElement).value === "0")).toBe(true);
    expect(screen.getByRole("button", { name: "Open this channel full-size: 4 · Camera 4" })).toBeInTheDocument();
  });
});
