import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import ClipExport from "@/features/device-dashboard/ClipExport";

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

const createObjectURL = vi.fn(() => "blob:clip");
const revokeObjectURL = vi.fn();

function setRange(start: string, end: string): void {
  fireEvent.change(screen.getByLabelText("From"), { target: { value: start } });
  fireEvent.change(screen.getByLabelText("To"), { target: { value: end } });
}

describe("ClipExport", () => {
  beforeEach(() => {
    notifyMock.mockReset();
    vi.restoreAllMocks();
    vi.stubGlobal("fetch", vi.fn());
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL,
    });
    createObjectURL.mockClear();
    revokeObjectURL.mockClear();
  });

  it("rejects invalid, reversed, and oversized ranges before contacting the device", () => {
    render(<ClipExport targetId="target-1" channelCount={1} defaultEnd="2026-07-22T08:00:00Z" />);
    fireEvent.click(screen.getByRole("button", { name: "Export video…" }));
    const download = screen.getByRole("button", { name: "Download clip" });

    setRange("invalid", "2026-07-22T08:00");
    fireEvent.click(download);
    expect(notifyMock).toHaveBeenLastCalledWith("error", "Enter a valid time range.", {
      source: "connections",
    });

    setRange("2026-07-22T08:01", "2026-07-22T08:00");
    fireEvent.click(download);
    expect(notifyMock).toHaveBeenLastCalledWith("error", "End must be after start.", {
      source: "connections",
    });

    setRange("2026-07-22T05:00", "2026-07-22T08:00");
    fireEvent.click(download);
    expect(notifyMock).toHaveBeenLastCalledWith(
      "error",
      "The window is longer than 2 hours; pick a shorter range.",
      { source: "connections" },
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("downloads the selected camera interval as an MP4 blob", async () => {
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    vi.mocked(fetch).mockResolvedValue(
      new Response(new Blob(["video"], { type: "video/mp4" }), { status: 200 }),
    );
    render(<ClipExport targetId="target/id" channelCount={3} defaultEnd="2026-07-22T08:00:00Z" />);
    fireEvent.click(screen.getByRole("button", { name: "Export video…" }));

    fireEvent.change(screen.getByLabelText("Camera"), { target: { value: "2" } });
    setRange("2026-07-22T07:59", "2026-07-22T08:00");
    fireEvent.click(screen.getByRole("button", { name: "Download clip" }));

    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    const request = vi.mocked(fetch).mock.calls[0]?.[0] as Request;
    expect(request).toBeInstanceOf(Request);
    expect(new URL(request.url).pathname).toBe("/api/v1/cctv/target%2Fid/export");
    expect(new URL(request.url).searchParams.get("channel")).toBe("2");
    expect(request.credentials).toBe("same-origin");
    await waitFor(() => expect(clickSpy).toHaveBeenCalledTimes(1));
    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:clip");
    expect(screen.getByRole("button", { name: "Export video…" })).toBeEnabled();
  });

  it("surfaces the canonical server error instead of downloading its JSON body", async () => {
    vi.mocked(fetch).mockResolvedValue(
      new Response(JSON.stringify({ message: "No retained footage" }), {
        status: 409,
        statusText: "Conflict",
        headers: { "Content-Type": "application/json" },
      }),
    );
    render(<ClipExport targetId="target-1" channelCount={null} defaultEnd="2026-07-22T08:00:00Z" />);
    fireEvent.click(screen.getByRole("button", { name: "Export video…" }));
    setRange("2026-07-22T07:59", "2026-07-22T08:00");

    fireEvent.click(screen.getByRole("button", { name: "Download clip" }));

    await waitFor(() =>
      expect(notifyMock).toHaveBeenCalledWith(
        "error",
        "Export failed: No retained footage",
        { source: "connections" },
      ),
    );
    expect(createObjectURL).not.toHaveBeenCalled();
  });
});
