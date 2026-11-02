import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import ArchivePlayer from "./ArchivePlayer";
import { playbackUrl } from "./archiveWindow";
import type { DeviceChannel } from "./api";
import type * as DeviceApi from "./api";

const apiMock = vi.hoisted(() => ({ searchDeviceRecordings: vi.fn() }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof DeviceApi>();
  return { ...actual, searchDeviceRecordings: apiMock.searchDeviceRecordings };
});

const channels: DeviceChannel[] = [
  { index: 1, name: "Entrance", enabled: true, ptz_support: "unknown", recording: null },
  { index: 2, name: "Yard", enabled: true, ptz_support: "unknown", recording: null },
];

const retention = { oldest: "2026-07-25T00:00:00Z", newest: "2026-07-25T10:00:00Z" };

function renderPlayer(overrides: Partial<React.ComponentProps<typeof ArchivePlayer>> = {}) {
  apiMock.searchDeviceRecordings.mockResolvedValue({ spans: [] });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ArchivePlayer
        targetId="target-1"
        channels={channels}
        retention={retention}
        spans={[{ start: "2026-07-25T00:00:00Z", end: "2026-07-25T10:00:00Z" }]}
        canExport
        {...overrides}
      />
    </QueryClientProvider>,
  );
}

describe("playbackUrl", () => {
  it("encodes the target, channel and RFC3339 window", () => {
    const url = playbackUrl(
      "",
      "target-1",
      3,
      Date.parse("2026-07-25T04:00:00Z"),
      Date.parse("2026-07-25T04:01:00Z"),
    );

    expect(url).toContain("/api/v1/cctv/target-1/playback");
    expect(url).toContain("channel=3");
    expect(url).toContain(encodeURIComponent("2026-07-25T04:00:00.000Z"));
  });
});

describe("ArchivePlayer", () => {
  /// An archive request costs a device session, so it must never happen just
  /// because the tab was opened.
  it("requests nothing until the operator asks to watch", () => {
    renderPlayer();

    expect(document.querySelector("video")).toBeNull();
    expect(screen.getByText(/press watch/i)).toBeInTheDocument();
  });

  it("loads the chosen window when Watch is pressed", () => {
    renderPlayer();

    fireEvent.click(screen.getByRole("button", { name: /watch/i }));

    const video = document.querySelector("video");
    expect(video).not.toBeNull();
    expect(video?.getAttribute("src")).toContain("/playback");
  });

  it("refuses an inverted window instead of requesting it", () => {
    renderPlayer();

    const start = screen.getByLabelText(/from/i);
    fireEvent.change(start, { target: { value: "2026-07-25T09:00" } });
    const end = screen.getByLabelText(/to/i);
    fireEvent.change(end, { target: { value: "2026-07-25T08:00" } });

    expect(screen.getByRole("alert")).toHaveTextContent(/after start/i);
    expect(screen.getByRole("button", { name: /watch/i })).toBeDisabled();
  });

  /// Requesting a gap yields an empty stream and an opaque failure, so the
  /// operator is warned before spending a device session on it.
  it("warns when the device reports no recording in the window", () => {
    renderPlayer({
      spans: [{ start: "2026-07-25T00:00:00Z", end: "2026-07-25T01:00:00Z" }],
    });

    const start = screen.getByLabelText(/from/i);
    // Local-time inputs against UTC spans: pick a gap wide enough that the
    // window lands inside it from any offset the test host may run in.
    fireEvent.change(start, { target: { value: "2026-07-26T12:00" } });
    const end = screen.getByLabelText(/to/i);
    fireEvent.change(end, { target: { value: "2026-07-26T12:10" } });

    expect(screen.getByText(/no recording in this window/i)).toBeInTheDocument();
  });

  it("hands the selected window to the export flow", () => {
    const onExport = vi.fn();
    renderPlayer({ onExport });

    fireEvent.click(screen.getByRole("button", { name: /export/i }));

    expect(onExport).toHaveBeenCalledTimes(1);
    const call = onExport.mock.calls[0] as [number, number, number];
    expect(call[0]).toBe(1);
    expect(call[2]).toBeGreaterThan(call[1]);
  });

  it("hides the export control when the caller may not export", () => {
    renderPlayer({ canExport: false });

    expect(screen.queryByRole("button", { name: /export/i })).not.toBeInTheDocument();
  });

  /// These recorders commonly write H.265, which most browsers refuse to
  /// decode. The first failure must not be a dead end: retry once asking the
  /// server to re-encode, since a copy is unwatchable for the operator.
  it("retries with a server-side transcode when the browser cannot decode", () => {
    renderPlayer();

    fireEvent.click(screen.getByRole("button", { name: /watch/i }));
    const first = document.querySelector("video");
    expect(first?.getAttribute("src")).not.toContain("transcode=true");

    fireEvent.error(first as Element);

    const retried = document.querySelector("video");
    expect(retried?.getAttribute("src")).toContain("transcode=true");
    expect(screen.queryByText(/cannot play the recording's format/i)).not.toBeInTheDocument();
  });

  /// Transcoding is the last resort — if the re-encoded stream also fails, the
  /// operator gets an honest message and the export escape hatch.
  it("offers the export escape hatch when even the transcode cannot be played", async () => {
    renderPlayer();

    fireEvent.click(screen.getByRole("button", { name: /watch/i }));
    fireEvent.error(document.querySelector("video") as Element);
    fireEvent.error(document.querySelector("video") as Element);

    expect(await screen.findByText(/cannot play the recording's format/i)).toBeInTheDocument();
  });

  /// A backgrounded tab must not hold a device session open.
  it("drops the stream when the surrounding tab goes inactive", () => {
    const { rerender } = renderPlayer();

    fireEvent.click(screen.getByRole("button", { name: /watch/i }));
    expect(document.querySelector("video")).not.toBeNull();

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    rerender(
      <QueryClientProvider client={client}>
        <ArchivePlayer
          targetId="target-1"
          channels={channels}
          retention={retention}
          spans={[]}
          canExport
          active={false}
        />
      </QueryClientProvider>,
    );

    expect(document.querySelector("video")).toBeNull();
  });

  /// Changing the camera must not re-request the archive on its own: an archive
  /// request opens a device session, and a render side effect would spend one
  /// every time the dropdown moves.
  it("keeps playing the camera it was started with when the dropdown changes", () => {
    renderPlayer();

    fireEvent.click(screen.getByRole("button", { name: /watch/i }));
    const before = document.querySelector("video")?.getAttribute("src");
    expect(before).toContain("channel=1");

    fireEvent.change(screen.getByLabelText(/camera/i), { target: { value: "2" } });

    expect(document.querySelector("video")?.getAttribute("src")).toBe(before);
  });

  it("offers a camera picker only when the device has several channels", () => {
    const { unmount } = renderPlayer();
    expect(screen.getByLabelText(/camera/i)).toBeInTheDocument();
    unmount();

    renderPlayer({ channels: channels.slice(0, 1) });
    expect(screen.queryByLabelText(/camera/i)).not.toBeInTheDocument();
  });
});
