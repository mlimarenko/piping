import { describe, expect, it } from "vitest";
import { MjpegMultipartParser, MjpegParserError } from "./mjpegMultipartParser";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function frameParts(boundary: string, body: string): Uint8Array {
  return encoder.encode(
    `--${boundary}\r\nContent-Type: image/jpeg\r\nContent-Length: ${body.length}\r\n\r\n${body}\r\n`,
  );
}

describe("MjpegMultipartParser", () => {
  it("parses content-length frames split across chunks", () => {
    const parser = new MjpegMultipartParser("camera");
    const bytes = frameParts("camera", "first-frame");

    expect(parser.push(bytes.slice(0, 11))).toEqual([]);
    expect(parser.push(bytes.slice(11, 33))).toEqual([]);
    const frames = parser.push(bytes.slice(33));

    expect(frames).toHaveLength(1);
    expect(decoder.decode(frames[0])).toBe("first-frame");
  });

  it("rejects a frame larger than the configured bound before buffering its body", () => {
    const parser = new MjpegMultipartParser("camera", {
      maxFrameBytes: 4,
      maxBufferBytes: 128,
    });

    expect(() => parser.push(frameParts("camera", "oversized"))).toThrow(
      new MjpegParserError("frame-too-large"),
    );
  });

  it("rejects streams whose unparsed buffer exceeds its bound", () => {
    const parser = new MjpegMultipartParser("camera", {
      maxFrameBytes: 64,
      maxBufferBytes: 64,
    });

    expect(() =>
      parser.push(
        encoder.encode(
          "--camera\r\nContent-Type: image/jpeg\r\nContent-Length: 64\r\n\r\npartial-body",
        ),
      ),
    ).toThrow(new MjpegParserError("buffer-too-large"));
  });
});
