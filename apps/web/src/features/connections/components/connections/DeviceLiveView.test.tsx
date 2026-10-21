import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DeviceLiveView from "./DeviceLiveView";

const createObjectURL = vi.fn<(blob: Blob) => string>();
const revokeObjectURL = vi.fn<(url: string) => void>();
const encoder = new TextEncoder();

function hangingMjpegResponse(frame = "jpeg-frame"): Response {
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(
        encoder.encode(
          `--camera\r\nContent-Type: image/jpeg\r\nContent-Length: ${frame.length}\r\n\r\n${frame}\r\n`,
        ),
      );
    },
  });
  return new Response(body, {
    headers: { "Content-Type": "multipart/x-mixed-replace; boundary=camera" },
  });
}

describe("DeviceLiveView MJPEG lifecycle", () => {
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

  it("classifies the first canonical busy response before reading multipart data", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        Response.json(
          {
            code: "CONNECTION_FAILED",
            reason_code: "CONNECTION_BUSY",
            message: "PRIVATE SLOT DETAIL",
          },
          { status: 429 },
        ),
      ),
    );

    render(<DeviceLiveView targetId="device-1" />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Connection capacity is busy");
    expect(screen.queryByText("PRIVATE SLOT DETAIL")).not.toBeInTheDocument();
  });

  it("publishes parsed frames as object URLs and revokes them on replacement", async () => {
    let streamController: ReadableStreamDefaultController<Uint8Array> | undefined;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        streamController = controller;
      },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(body, {
          headers: { "Content-Type": "multipart/x-mixed-replace; boundary=camera" },
        }),
      ),
    );
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");

    render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(streamController).toBeDefined());

    streamController?.enqueue(
      encoder.encode("--camera\r\nContent-Type: image/jpeg\r\nContent-Length: 3\r\n\r\none\r\n"),
    );
    expect(await screen.findByRole("img", { name: "Live video" })).toHaveAttribute(
      "src",
      "blob:frame-1",
    );

    streamController?.enqueue(
      encoder.encode("--camera\r\nContent-Type: image/jpeg\r\nContent-Length: 3\r\n\r\ntwo\r\n"),
    );
    await waitFor(() => expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-1"));
    expect(screen.getByRole("img", { name: "Live video" })).toHaveAttribute(
      "src",
      "blob:frame-2",
    );
  });

  it("aborts on channel switch, backgrounding, and unmount, revoking frame URLs", async () => {
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");
    const signals: AbortSignal[] = [];
    const fetchMock = vi.fn((request: Request): Promise<Response> => {
      signals.push(request.signal);
      return Promise.resolve(hangingMjpegResponse());
    });
    vi.stubGlobal("fetch", fetchMock);

    const { rerender, unmount } = render(<DeviceLiveView targetId="device-1" channel={1} />);
    expect(await screen.findByRole("img", { name: "Live video" })).toHaveAttribute(
      "src",
      "blob:frame-1",
    );

    rerender(<DeviceLiveView targetId="device-1" channel={2} />);
    await waitFor(() => expect(signals).toHaveLength(2));
    expect(signals[0]?.aborted).toBe(true);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-1");

    rerender(<DeviceLiveView targetId="device-1" channel={2} active={false} />);
    expect(signals[1]?.aborted).toBe(true);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-2");

    rerender(<DeviceLiveView targetId="device-1" channel={2} active />);
    await waitFor(() => expect(signals).toHaveLength(3));
    unmount();
    expect(signals[2]?.aborted).toBe(true);
  });
});
