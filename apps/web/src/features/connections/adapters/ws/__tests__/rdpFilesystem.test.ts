import { afterEach, describe, expect, it, vi } from "vitest";

const INDEX_MIMETYPE = "application/vnd.glyptodon.guacamole.stream-index+json";
const ROOT_STREAM = "/";
const DEFAULT_BLOB_LENGTH = 6048;

interface MockInputStream {
  onblob?: (data64: string) => void;
  onend?: () => void;
  sendAck: ReturnType<typeof vi.fn>;
  stringReader?: { emitText(text: string): void; end(): void };
  blobReader?: { progress(length: number): void; complete(blob?: Blob): void };
}

const guacMocks = vi.hoisted(() => {
  class MockStringReader {
    public ontext: ((text: string) => void) | null = null;
    public onend: (() => void) | null = null;

    constructor(stream: MockInputStream) {
      stream.stringReader = this;
    }

    emitText(text: string): void {
      this.ontext?.(text);
    }

    end(): void {
      this.onend?.();
    }
  }

  class MockBlobReader {
    public static instances: MockBlobReader[] = [];
    public onprogress: ((length: number) => void) | null = null;
    public onend: (() => void) | null = null;
    private blob: Blob;

    constructor(stream: MockInputStream, mimetype: string) {
      stream.blobReader = this;
      this.blob = new Blob([], { type: mimetype });
      MockBlobReader.instances.push(this);
    }

    getBlob(): Blob {
      return this.blob;
    }

    progress(length: number): void {
      this.onprogress?.(length);
    }

    complete(blob?: Blob): void {
      if (blob) this.blob = blob;
      this.onend?.();
    }
  }

  class MockBlobWriter {
    public static instances: MockBlobWriter[] = [];
    public onprogress: ((blob: Blob, offset: number) => void) | null = null;
    public oncomplete: ((blob: Blob) => void) | null = null;
    public onerror: ((blob: Blob, offset: number, error: Error) => void) | null = null;
    public onack: ((status: { message?: string; isError(): boolean }) => void) | null = null;
    public sendBlob = vi.fn();
    public sendEnd = vi.fn();

    constructor() {
      MockBlobWriter.instances.push(this);
    }
  }

  return { MockStringReader, MockBlobReader, MockBlobWriter };
});

vi.mock("guacamole-common-js", () => ({
  default: {
    Object: {
      ROOT_STREAM: "/",
      STREAM_INDEX_MIMETYPE: "application/vnd.glyptodon.guacamole.stream-index+json",
    },
    ArrayBufferWriter: {
      DEFAULT_BLOB_LENGTH: 6048,
    },
    StringReader: guacMocks.MockStringReader,
    BlobReader: guacMocks.MockBlobReader,
    BlobWriter: guacMocks.MockBlobWriter,
  },
}));

import type Guacamole from "guacamole-common-js";
import { createRdpFilesystem, joinDrivePath, type RdpFsEntry } from "../rdpFilesystem";

class MockDriveObject {
  public onbody: ((stream: MockInputStream, mimetype: string, name: string) => void) | null = null;
  public requestInputStream = vi.fn();
  public createOutputStream = vi.fn((mimetype: string, name: string) => ({ mimetype, name }));

  emitBody(name: string, mimetype: string): MockInputStream {
    const stream: MockInputStream = { sendAck: vi.fn() };
    this.onbody?.(stream, mimetype, name);
    return stream;
  }
}

function makeFilesystem() {
  const drive = new MockDriveObject();
  const filesystem = createRdpFilesystem(
    drive as unknown as Guacamole.Object,
    "Piping",
  );
  return { drive, filesystem };
}

describe("joinDrivePath", () => {
  it("joins root and nested paths without duplicate slashes", () => {
    expect(joinDrivePath("/", "file.txt")).toBe("/file.txt");
    expect(joinDrivePath("/Download/", "file.txt")).toBe("/Download/file.txt");
  });
});

describe("createRdpFilesystem", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
    guacMocks.MockBlobReader.instances = [];
    guacMocks.MockBlobWriter.instances = [];
  });

  it("queues a same-path retry until the stale timed-out body is drained", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();

    const first = filesystem.list(ROOT_STREAM);
    const firstResult = first.catch((error: Error) => error);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);
    expect(drive.requestInputStream).toHaveBeenLastCalledWith(ROOT_STREAM);

    vi.advanceTimersByTime(20_000);
    await expect(firstResult).resolves.toMatchObject({
      message: "Directory listing timed out: /",
    });

    const second = filesystem.list(ROOT_STREAM);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    const stale = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    stale.onblob?.("stale-index");
    expect(stale.sendAck).toHaveBeenCalledWith("OK", 0);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    stale.onend?.();
    expect(drive.requestInputStream).toHaveBeenCalledTimes(2);
    expect(drive.requestInputStream).toHaveBeenLastCalledWith(ROOT_STREAM);

    const fresh = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    fresh.stringReader?.emitText(
      JSON.stringify({
        "/z.txt": "text/plain",
        "/Download": INDEX_MIMETYPE,
        "/a.txt": "text/plain",
      }),
    );
    expect(fresh.sendAck).toHaveBeenCalledWith("OK", 0);
    fresh.stringReader?.end();

    await expect(second).resolves.toEqual([
      {
        name: "Download",
        path: "/Download",
        mimetype: INDEX_MIMETYPE,
        isDirectory: true,
      },
      {
        name: "a.txt",
        path: "/a.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
      {
        name: "z.txt",
        path: "/z.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
    ]);
  });

  it("dispatches a queued same-path retry after reconcile grace if the stale body never arrives", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();

    const firstResult = filesystem.list(ROOT_STREAM).catch((error: Error) => error);
    vi.advanceTimersByTime(20_000);
    await expect(firstResult).resolves.toMatchObject({
      message: "Directory listing timed out: /",
    });

    const second = filesystem.list(ROOT_STREAM);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(1_999);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(1);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(2);
    expect(drive.requestInputStream).toHaveBeenLastCalledWith(ROOT_STREAM);

    const fresh = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    fresh.stringReader?.emitText(JSON.stringify({ "/uploaded.txt": "text/plain" }));
    expect(fresh.sendAck).toHaveBeenCalledWith("OK", 0);
    fresh.stringReader?.end();

    await expect(second).resolves.toEqual([
      {
        name: "uploaded.txt",
        path: "/uploaded.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
    ]);
  });

  it("starts the body-arrival timeout only when a queued request is dispatched", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();

    const first = filesystem.list(ROOT_STREAM);
    const secondResult = filesystem.list(ROOT_STREAM).catch((error: Error) => error);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(19_999);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    const firstStream = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    firstStream.stringReader?.emitText(JSON.stringify({ "/first.txt": "text/plain" }));
    expect(firstStream.sendAck).toHaveBeenCalledWith("OK", 0);
    firstStream.stringReader?.end();
    await expect(first).resolves.toHaveLength(1);

    expect(drive.requestInputStream).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(19_999);
    await Promise.resolve();
    expect(await Promise.race([secondResult, Promise.resolve("pending")])).toBe("pending");

    vi.advanceTimersByTime(1);
    await expect(secondResult).resolves.toMatchObject({
      message: "Directory listing timed out: /",
    });
  });

  it("acknowledges the directory body and each index chunk so guacd can finish the stream", async () => {
    const { drive, filesystem } = makeFilesystem();

    const listing = filesystem.list(ROOT_STREAM);
    const stream = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    expect(stream.sendAck).toHaveBeenCalledTimes(1);
    expect(stream.sendAck).toHaveBeenNthCalledWith(1, "OK", 0);

    stream.stringReader?.emitText("{\"/a.txt\":\"text/plain\"");
    stream.stringReader?.emitText(",\"/b.txt\":\"text/plain\"}");
    expect(stream.sendAck).toHaveBeenCalledTimes(3);
    expect(stream.sendAck).toHaveBeenNthCalledWith(1, "OK", 0);
    expect(stream.sendAck).toHaveBeenNthCalledWith(2, "OK", 0);
    expect(stream.sendAck).toHaveBeenNthCalledWith(3, "OK", 0);

    stream.stringReader?.end();

    await expect(listing).resolves.toEqual([
      {
        name: "a.txt",
        path: "/a.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
      {
        name: "b.txt",
        path: "/b.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
    ]);
  });

  it("resolves an empty directory after accepting the body stream", async () => {
    const { drive, filesystem } = makeFilesystem();

    const listing = filesystem.list(ROOT_STREAM);
    const stream = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);

    expect(stream.sendAck).toHaveBeenCalledTimes(1);
    stream.stringReader?.emitText("{}");
    stream.stringReader?.end();

    await expect(listing).resolves.toEqual([]);
    expect(stream.sendAck).toHaveBeenCalledTimes(2);
  });

  it("accepts and drains an unclaimed body without blocking later requests", async () => {
    const { drive, filesystem } = makeFilesystem();

    const unclaimed = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    expect(unclaimed.sendAck).toHaveBeenCalledTimes(1);
    unclaimed.onblob?.("orphaned-index");
    unclaimed.onend?.();
    expect(unclaimed.sendAck).toHaveBeenCalledTimes(2);

    const listing = filesystem.list(ROOT_STREAM);
    expect(drive.requestInputStream).toHaveBeenCalledTimes(1);

    const stream = drive.emitBody(ROOT_STREAM, INDEX_MIMETYPE);
    stream.stringReader?.emitText(JSON.stringify({ "/ready.txt": "text/plain" }));
    stream.stringReader?.end();

    await expect(listing).resolves.toEqual([
      {
        name: "ready.txt",
        path: "/ready.txt",
        mimetype: "text/plain",
        isDirectory: false,
      },
    ]);
  });

  it("rejects a listing with a non-directory body while draining the stream", async () => {
    const { drive, filesystem } = makeFilesystem();

    const result = filesystem.list(ROOT_STREAM).catch((error: Error) => error);
    const stream = drive.emitBody(ROOT_STREAM, "text/plain");

    expect(stream.sendAck).toHaveBeenCalledTimes(1);
    stream.onblob?.("not-an-index");
    stream.onend?.();

    await expect(result).resolves.toMatchObject({
      message: "Not a directory: /",
    });
    expect(stream.sendAck).toHaveBeenCalledTimes(2);
  });

  it("rejects downloads on timeout instead of staying pending", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();
    const entry: RdpFsEntry = {
      name: "report.txt",
      path: "/report.txt",
      mimetype: "text/plain",
      isDirectory: false,
    };

    const result = filesystem.download(entry).catch((error: Error) => error);
    expect(drive.requestInputStream).toHaveBeenCalledWith("/report.txt");

    vi.advanceTimersByTime(20_000);
    await expect(result).resolves.toMatchObject({
      message: "Download timed out: /report.txt",
    });
  });

  it("keeps an active download alive after the body arrives while data progresses", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();
    const entry: RdpFsEntry = {
      name: "large.bin",
      path: "/large.bin",
      mimetype: "application/octet-stream",
      isDirectory: false,
    };

    const download = filesystem.download(entry);
    const stream = drive.emitBody("/large.bin", "application/octet-stream");
    expect(stream.sendAck).toHaveBeenCalledTimes(1);

    vi.advanceTimersByTime(19_000);
    stream.blobReader?.progress(1024);
    vi.advanceTimersByTime(19_000);
    stream.blobReader?.progress(1024);
    vi.advanceTimersByTime(19_000);
    stream.blobReader?.complete(new Blob(["done"], { type: "application/octet-stream" }));

    await expect(download).resolves.toMatchObject({
      size: 4,
      type: "application/octet-stream",
    });
  });

  it("rejects downloads when the body stream stalls after arrival", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();
    const entry: RdpFsEntry = {
      name: "stalled.bin",
      path: "/stalled.bin",
      mimetype: "application/octet-stream",
      isDirectory: false,
    };

    const result = filesystem.download(entry).catch((error: Error) => error);
    drive.emitBody("/stalled.bin", "application/octet-stream");

    vi.advanceTimersByTime(20_000);
    await expect(result).resolves.toMatchObject({
      message: "Download timed out: /stalled.bin",
    });
  });

  it("downloads blob contents from a file body stream", async () => {
    vi.useFakeTimers();
    const { drive, filesystem } = makeFilesystem();
    const entry: RdpFsEntry = {
      name: "report.txt",
      path: "/report.txt",
      mimetype: "text/plain",
      isDirectory: false,
    };

    const download = filesystem.download(entry);
    const stream = drive.emitBody("/report.txt", "text/plain");
    expect(stream.sendAck).toHaveBeenCalledTimes(1);
    stream.blobReader?.complete(new Blob(["hello"], { type: "text/plain" }));

    const blob = await download;
    expect(blob.type).toBe("text/plain");
    await expect(blob.text()).resolves.toBe("hello");
  });

  it("closes upload streams with sendEnd before resolving and normalizes progress", async () => {
    const { drive, filesystem } = makeFilesystem();
    const progress = vi.fn();
    const file = new File([new Uint8Array(7000)], "upload.bin", {
      type: "application/octet-stream",
    });
    const resolved: string[] = [];

    const upload = filesystem.upload(ROOT_STREAM, file, progress).then(() => {
      resolved.push("resolved");
    });

    const writer = guacMocks.MockBlobWriter.instances[0];
    expect(drive.createOutputStream).toHaveBeenCalledWith(
      "application/octet-stream",
      "/upload.bin",
    );
    expect(writer?.sendBlob).toHaveBeenCalledWith(file);

    writer?.onprogress?.(file, 0);
    writer?.onprogress?.(file, DEFAULT_BLOB_LENGTH);
    writer?.onprogress?.(file, 0);
    expect(progress.mock.calls.map(([loaded]) => loaded)).toEqual([
      DEFAULT_BLOB_LENGTH,
      7000,
      7000,
    ]);

    writer?.oncomplete?.(file);
    expect(writer?.sendEnd).toHaveBeenCalledOnce();
    expect(progress).toHaveBeenLastCalledWith(7000, 7000);
    expect(resolved).toEqual([]);

    await upload;
    expect(resolved).toEqual(["resolved"]);
  });

  it("rejects uploads on remote error acknowledgements", async () => {
    const { filesystem } = makeFilesystem();
    const file = new File([new Uint8Array(16)], "denied.bin", {
      type: "application/octet-stream",
    });

    const upload = filesystem.upload(ROOT_STREAM, file).catch((error: Error) => error);
    const writer = guacMocks.MockBlobWriter.instances[0];

    writer?.onack?.({
      message: "permission denied",
      isError: () => true,
    });

    await expect(upload).resolves.toMatchObject({
      message: "Upload failed: permission denied",
    });
    expect(writer?.sendEnd).not.toHaveBeenCalled();
  });

  it("rejects uploads when the writer stops receiving acknowledgements", async () => {
    vi.useFakeTimers();
    const { filesystem } = makeFilesystem();
    const file = new File([new Uint8Array(16)], "stalled.bin", {
      type: "application/octet-stream",
    });

    const result = filesystem.upload(ROOT_STREAM, file).catch((error: Error) => error);

    vi.advanceTimersByTime(20_000);
    await expect(result).resolves.toMatchObject({
      message: "Upload timed out: /stalled.bin",
    });
  });
});
