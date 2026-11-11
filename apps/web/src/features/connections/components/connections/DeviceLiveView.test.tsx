import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DeviceLiveView from "./DeviceLiveView";

const createObjectURL = vi.fn<(blob: Blob) => string>();
const revokeObjectURL = vi.fn<(url: string) => void>();
const encoder = new TextEncoder();

function mjpegPart(frame: string): string {
  return `--camera\r\nContent-Type: image/jpeg\r\nContent-Length: ${frame.length}\r\n\r\n${frame}\r\n`;
}

function hangingMjpegResponse(frame = "jpeg-frame"): Response {
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(encoder.encode(mjpegPart(frame)));
    },
  });
  return new Response(body, {
    headers: { "Content-Type": "application/octet-stream; boundary=camera" },
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

  it("publishes parsed frames as object URLs and revokes a replaced frame only after its successor loads", async () => {
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
          headers: { "Content-Type": "application/octet-stream; boundary=camera" },
        }),
      ),
    );
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");

    render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(streamController).toBeDefined());

    streamController?.enqueue(encoder.encode(mjpegPart("one")));
    const image = await screen.findByRole("img", { name: "Live video" });
    expect(image).toHaveAttribute("src", "blob:frame-1");
    fireEvent.load(image);

    streamController?.enqueue(encoder.encode(mjpegPart("two")));
    await waitFor(() => expect(image).toHaveAttribute("src", "blob:frame-2"));
    expect(revokeObjectURL).not.toHaveBeenCalled();

    fireEvent.load(image);
    expect(revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-1");
    expect(image).toHaveAttribute("src", "blob:frame-2");
  });

  it("collapses a multi-frame network chunk into one allocation", async () => {
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
          headers: { "Content-Type": "application/octet-stream; boundary=camera" },
        }),
      ),
    );
    let created = 0;
    createObjectURL.mockImplementation(() => `blob:frame-${(created += 1)}`);

    render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(streamController).toBeDefined());

    streamController?.enqueue(encoder.encode(mjpegPart("one")));
    const image = await screen.findByRole("img", { name: "Live video" });
    expect(image).toHaveAttribute("src", "blob:frame-1");
    fireEvent.load(image);

    streamController?.enqueue(
      encoder.encode(mjpegPart("two") + mjpegPart("three") + mjpegPart("four")),
    );
    await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(2));

    expect(revokeObjectURL).not.toHaveBeenCalled();
    await waitFor(() => expect(image).toHaveAttribute("src", "blob:frame-2"));
    const publishedFrame = createObjectURL.mock.calls[1]?.[0];
    expect(publishedFrame).toBeInstanceOf(Blob);
    expect(await publishedFrame?.text()).toBe("four");

    fireEvent.load(image);
    expect(revokeObjectURL).toHaveBeenCalledTimes(1);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-1");
  });

  it("revokes a frame superseded before it paints without touching the painted frame", async () => {
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
          headers: { "Content-Type": "application/octet-stream; boundary=camera" },
        }),
      ),
    );
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");

    const { unmount } = render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(streamController).toBeDefined());

    streamController?.enqueue(encoder.encode(mjpegPart("one")));
    const image = await screen.findByRole("img", { name: "Live video" });
    expect(image).toHaveAttribute("src", "blob:frame-1");

    streamController?.enqueue(encoder.encode(mjpegPart("two")));
    await waitFor(() => expect(image).toHaveAttribute("src", "blob:frame-2"));
    expect(revokeObjectURL.mock.calls).toEqual([["blob:frame-1"]]);

    unmount();
    expect(revokeObjectURL.mock.calls).toEqual([["blob:frame-1"], ["blob:frame-2"]]);
  });

  it("revokes the painted frame when the view is hidden", async () => {
    createObjectURL.mockReturnValueOnce("blob:frame-1");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(hangingMjpegResponse()));

    const { rerender } = render(<DeviceLiveView targetId="device-1" />);
    const image = await screen.findByRole("img", { name: "Live video" });
    fireEvent.load(image);

    rerender(<DeviceLiveView targetId="device-1" active={false} />);
    expect(revokeObjectURL.mock.calls).toEqual([["blob:frame-1"]]);
    expect(screen.queryByRole("img", { name: "Live video" })).not.toBeInTheDocument();
  });

  it("never restores a revoked frame URL when the view resumes", async () => {
    const controllers: ReadableStreamDefaultController<Uint8Array>[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(() => {
        const body = new ReadableStream<Uint8Array>({
          start(controller) {
            controllers.push(controller);
          },
        });
        return Promise.resolve(
          new Response(body, {
            headers: { "Content-Type": "application/octet-stream; boundary=camera" },
          }),
        );
      }),
    );
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");

    const { rerender } = render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(controllers).toHaveLength(1));
    controllers[0]?.enqueue(encoder.encode(mjpegPart("one")));
    const image = await screen.findByRole("img", { name: "Live video" });
    expect(image).toHaveAttribute("src", "blob:frame-1");
    fireEvent.load(image);

    rerender(<DeviceLiveView targetId="device-1" active={false} />);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:frame-1");
    expect(screen.queryByRole("img", { name: "Live video" })).not.toBeInTheDocument();

    rerender(<DeviceLiveView targetId="device-1" active />);
    await waitFor(() => expect(controllers).toHaveLength(2));
    expect(screen.queryByRole("img", { name: "Live video" })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    controllers[1]?.enqueue(encoder.encode(mjpegPart("two")));
    expect(await screen.findByRole("img", { name: "Live video" })).toHaveAttribute(
      "src",
      "blob:frame-2",
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("clears a media failure once the current stream delivers another frame", async () => {
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
          headers: { "Content-Type": "application/octet-stream; boundary=camera" },
        }),
      ),
    );
    createObjectURL.mockReturnValueOnce("blob:frame-1").mockReturnValueOnce("blob:frame-2");

    render(<DeviceLiveView targetId="device-1" />);
    await waitFor(() => expect(streamController).toBeDefined());
    streamController?.enqueue(encoder.encode(mjpegPart("one")));
    const image = await screen.findByRole("img", { name: "Live video" });
    fireEvent.load(image);
    fireEvent.error(image);
    expect(await screen.findByRole("alert")).toBeInTheDocument();

    streamController?.enqueue(encoder.encode(mjpegPart("two")));
    await waitFor(() =>
      expect(screen.getByRole("img", { name: "Live video" })).toHaveAttribute(
        "src",
        "blob:frame-2",
      ),
    );
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
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
