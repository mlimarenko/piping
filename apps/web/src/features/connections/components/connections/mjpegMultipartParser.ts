const HEADER_TERMINATOR = new Uint8Array([13, 10, 13, 10]);
const CRLF = new Uint8Array([13, 10]);
const DEFAULT_MAX_FRAME_BYTES = 4 * 1024 * 1024;
const DEFAULT_MAX_BUFFER_BYTES = DEFAULT_MAX_FRAME_BYTES + 128 * 1024;
const MAX_HEADER_BYTES = 16 * 1024;

export type MjpegParserErrorKind =
  | "buffer-too-large"
  | "frame-too-large"
  | "invalid-headers";

export class MjpegParserError extends Error {
  readonly kind: MjpegParserErrorKind;

  constructor(kind: MjpegParserErrorKind) {
    super(kind);
    this.name = "MjpegParserError";
    this.kind = kind;
  }
}

interface ParserOptions {
  maxFrameBytes?: number;
  maxBufferBytes?: number;
}

function concatBytes(left: Uint8Array, right: Uint8Array): Uint8Array {
  const result = new Uint8Array(left.byteLength + right.byteLength);
  result.set(left);
  result.set(right, left.byteLength);
  return result;
}

function indexOfBytes(haystack: Uint8Array, needle: Uint8Array, from = 0): number {
  const lastStart = haystack.byteLength - needle.byteLength;
  for (let index = from; index <= lastStart; index += 1) {
    let matches = true;
    for (let offset = 0; offset < needle.byteLength; offset += 1) {
      if (haystack[index + offset] !== needle[offset]) {
        matches = false;
        break;
      }
    }
    if (matches) return index;
  }
  return -1;
}

function contentLengthFromHeaders(headers: Uint8Array): number {
  const text = new TextDecoder("ascii", { fatal: true }).decode(headers);
  const match = /^content-length\s*:\s*(\d+)\s*$/im.exec(text);
  if (!match) throw new MjpegParserError("invalid-headers");
  const length = Number(match[1]);
  if (!Number.isSafeInteger(length) || length <= 0) {
    throw new MjpegParserError("invalid-headers");
  }
  return length;
}

/**
 * Incremental, bounded parser for ffmpeg's multipart MJPEG response. The
 * backend emits a Content-Length for every JPEG part, so binary JPEG bytes are
 * never searched for boundary-looking text.
 */
export class MjpegMultipartParser {
  private buffer: Uint8Array<ArrayBufferLike> = new Uint8Array();
  private readonly boundary: Uint8Array<ArrayBufferLike>;
  private readonly maxFrameBytes: number;
  private readonly maxBufferBytes: number;

  constructor(boundary: string, options: ParserOptions = {}) {
    if (!boundary || /[\r\n]/.test(boundary)) {
      throw new MjpegParserError("invalid-headers");
    }
    this.boundary = new TextEncoder().encode(`--${boundary}`);
    this.maxFrameBytes = options.maxFrameBytes ?? DEFAULT_MAX_FRAME_BYTES;
    this.maxBufferBytes = options.maxBufferBytes ?? DEFAULT_MAX_BUFFER_BYTES;
  }

  push(chunk: Uint8Array): Uint8Array[] {
    this.buffer = concatBytes(this.buffer, chunk);
    if (this.buffer.byteLength > this.maxBufferBytes) {
      throw new MjpegParserError("buffer-too-large");
    }

    const frames: Uint8Array[] = [];
    while (this.consumeFrame(frames)) {
      // Consume every complete frame currently buffered.
    }
    return frames;
  }

  private consumeFrame(frames: Uint8Array[]): boolean {
    const boundaryIndex = indexOfBytes(this.buffer, this.boundary);
    if (boundaryIndex < 0) {
      const retainedBytes = Math.max(0, this.boundary.byteLength - 1);
      if (this.buffer.byteLength > retainedBytes) {
        this.buffer = this.buffer.slice(this.buffer.byteLength - retainedBytes);
      }
      return false;
    }
    if (boundaryIndex > 0) this.buffer = this.buffer.slice(boundaryIndex);

    const headerStart = this.boundary.byteLength + CRLF.byteLength;
    if (this.buffer.byteLength < headerStart) return false;
    const headerEnd = indexOfBytes(this.buffer, HEADER_TERMINATOR, headerStart);
    if (headerEnd < 0) {
      if (this.buffer.byteLength > MAX_HEADER_BYTES) {
        throw new MjpegParserError("invalid-headers");
      }
      return false;
    }

    const frameLength = contentLengthFromHeaders(this.buffer.slice(headerStart, headerEnd));
    if (frameLength > this.maxFrameBytes) {
      throw new MjpegParserError("frame-too-large");
    }

    const frameStart = headerEnd + HEADER_TERMINATOR.byteLength;
    const frameEnd = frameStart + frameLength;
    if (this.buffer.byteLength < frameEnd) return false;

    frames.push(this.buffer.slice(frameStart, frameEnd));
    const trailingCrlf =
      this.buffer[frameEnd] === CRLF[0] && this.buffer[frameEnd + 1] === CRLF[1]
        ? CRLF.byteLength
        : 0;
    this.buffer = this.buffer.slice(frameEnd + trailingCrlf);
    return true;
  }
}

export function multipartBoundary(contentType: string | null): string | undefined {
  if (!contentType?.toLowerCase().startsWith("multipart/x-mixed-replace")) return undefined;
  const match = /(?:^|;)\s*boundary\s*=\s*(?:"([^"]+)"|([^;\s]+))/i.exec(contentType);
  return (match?.[1] ?? match?.[2])?.replace(/^--/, "");
}
