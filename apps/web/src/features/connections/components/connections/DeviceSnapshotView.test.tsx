import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DeviceSnapshotView from "./DeviceSnapshotView";

const createObjectURL = vi.fn<(blob: Blob) => string>();
const revokeObjectURL = vi.fn<(url: string) => void>();

describe("DeviceSnapshotView media lifecycle", () => {
  beforeEach(() => {
    createObjectURL.mockReset();
    revokeObjectURL.mockReset();
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL,
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("classifies a canonical authentication failure without exposing backend prose", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        Response.json(
          {
            code: "CONNECTION_FAILED",
            reason_code: "CONNECTION_AUTHENTICATION_FAILED",
            message: "PRIVATE DEVICE DETAIL",
          },
          { status: 503 },
        ),
      ),
    );

    render(<DeviceSnapshotView targetId="device-1" hideControls />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Authentication failed");
    expect(screen.queryByText("PRIVATE DEVICE DETAIL")).not.toBeInTheDocument();
  });

  it("fetches a blob and revokes each object URL on channel switch and unmount", async () => {
    createObjectURL.mockReturnValueOnce("blob:snapshot-1").mockReturnValueOnce("blob:snapshot-2");
    const fetchMock = vi.fn().mockResolvedValueOnce(new Response(new Blob(["first"], { type: "image/jpeg" })))
      .mockResolvedValueOnce(new Response(new Blob(["second"], { type: "image/jpeg" })));
    vi.stubGlobal("fetch", fetchMock);

    const { rerender, unmount } = render(
      <DeviceSnapshotView targetId="device-1" channel={1} hideControls />,
    );

    expect(await screen.findByRole("img", { name: "Device snapshot" })).toHaveAttribute(
      "src",
      "blob:snapshot-1",
    );
    expect((fetchMock.mock.calls[0]?.[0] as Request).credentials).toBe("include");

    rerender(<DeviceSnapshotView targetId="device-1" channel={2} hideControls />);

    await waitFor(() => expect(revokeObjectURL).toHaveBeenCalledWith("blob:snapshot-1"));
    expect(await screen.findByRole("img", { name: "Device snapshot" })).toHaveAttribute(
      "src",
      "blob:snapshot-2",
    );

    unmount();
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:snapshot-2");
  });

  it("aborts an in-flight request on channel switch, backgrounding, and unmount", async () => {
    const signals: AbortSignal[] = [];
    const fetchMock = vi.fn((request: Request) => {
      signals.push(request.signal);
      return new Promise<Response>(() => undefined);
    });
    vi.stubGlobal("fetch", fetchMock);

    const { rerender, unmount } = render(
      <DeviceSnapshotView targetId="device-1" channel={1} hideControls />,
    );
    await waitFor(() => expect(signals).toHaveLength(1));

    rerender(<DeviceSnapshotView targetId="device-1" channel={2} hideControls />);
    await waitFor(() => expect(signals).toHaveLength(2));
    expect(signals[0]?.aborted).toBe(true);

    rerender(<DeviceSnapshotView targetId="device-1" channel={2} hideControls active={false} />);
    expect(signals[1]?.aborted).toBe(true);

    rerender(<DeviceSnapshotView targetId="device-1" channel={2} hideControls active />);
    await waitFor(() => expect(signals).toHaveLength(3));
    unmount();
    expect(signals[2]?.aborted).toBe(true);
  });
});
