import { afterEach, describe, expect, it, vi } from "vitest";
import {
  deleteSshSessionFiles,
  ensureSshSessionDirectory,
  moveSshSessionFiles,
  preflightSshSessionFileDownload,
  previewSshSessionFile,
  sshSessionFileDownloadUrl,
  startSshSessionFileDownload,
  uploadSshSessionFile,
} from "./session-files-api";

class FakeUploadTarget {
  onprogress: ((event: ProgressEvent) => void) | null = null;
  onload: ((event: ProgressEvent) => void) | null = null;
}

class FakeXMLHttpRequest {
  static instances: FakeXMLHttpRequest[] = [];

  upload = new FakeUploadTarget();
  withCredentials = false;
  responseType = "";
  status = 0;
  statusText = "";
  responseText = "";
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;
  method = "";
  url = "";
  headers = new Map<string, string>();

  constructor() {
    FakeXMLHttpRequest.instances.push(this);
  }

  open(method: string, url: string) {
    this.method = method;
    this.url = url;
  }

  setRequestHeader(name: string, value: string) {
    this.headers.set(name, value);
  }

  send(_form?: FormData) {
    this.upload.onprogress?.({ loaded: 10, total: 10, lengthComputable: true } as ProgressEvent);
    this.upload.onload?.({ loaded: 10, total: 10, lengthComputable: true } as ProgressEvent);
    this.status = 200;
    this.responseText = JSON.stringify({ path: "/tmp/file.txt", bytes: 10, duration_ms: 5 });
    this.onload?.();
  }

  abort() {
    this.onabort?.();
  }
}

describe("session-files-api", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    FakeXMLHttpRequest.instances = [];
  });

  it("builds encoded browser download URLs", () => {
    expect(sshSessionFileDownloadUrl("session/1", "/tmp/file with spaces.txt")).toBe(
      "/api/v1/connections/sessions/session%2F1/files/download?path=%2Ftmp%2Ffile+with+spaces.txt",
    );
    expect(
      sshSessionFileDownloadUrl("session/1", "/tmp/reports", {
        archive: true,
      }),
    ).toBe(
      "/api/v1/connections/sessions/session%2F1/files/download?path=%2Ftmp%2Freports&archive=true",
    );
  });

  it("preflights downloads with JSON instead of reading the file blob", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          path: "/tmp/file.txt",
          file_name: "file.txt",
          bytes: 10,
          logical_bytes: 10,
          entry_count: 1,
          kind: "file",
          archive: false,
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(preflightSshSessionFileDownload("session-1", "/tmp/file.txt")).resolves.toEqual({
      path: "/tmp/file.txt",
      file_name: "file.txt",
      bytes: 10,
      logical_bytes: 10,
      entry_count: 1,
      kind: "file",
      archive: false,
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/download/preflight",
    );
    expect(new URL(request.url).searchParams.get("path")).toBe("/tmp/file.txt");
    expect(request.credentials).toBe("include");
  });

  it("preflights directory archive downloads explicitly", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          path: "/tmp/reports",
          file_name: "reports.tar.gz",
          bytes: null,
          logical_bytes: 4096,
          entry_count: 7,
          kind: "directory",
          archive: true,
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      preflightSshSessionFileDownload("session-1", "/tmp/reports", { archive: true }),
    ).resolves.toEqual({
      path: "/tmp/reports",
      file_name: "reports.tar.gz",
      bytes: null,
      logical_bytes: 4096,
      entry_count: 7,
      kind: "directory",
      archive: true,
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/download/preflight",
    );
    const params = new URL(request.url).searchParams;
    expect(params.get("path")).toBe("/tmp/reports");
    expect(params.get("archive")).toBe("true");
    expect(request.credentials).toBe("include");
  });

  it("ensures remote directories through a dedicated endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ path: "/tmp/reports", bytes: 0, duration_ms: 4 }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(ensureSshSessionDirectory("session-1", "/tmp/reports")).resolves.toEqual({
      path: "/tmp/reports",
      bytes: 0,
      duration_ms: 4,
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/mkdir",
    );
    expect(new URL(request.url).searchParams.get("path")).toBe("/tmp/reports");
    expect(request.method).toBe("POST");
    expect(request.credentials).toBe("include");
  });

  it("previews text files through a bounded JSON endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          path: "/tmp/app.log",
          bytes: 120,
          preview_bytes: 64,
          truncated: true,
          content: "hello",
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(previewSshSessionFile("session-1", "/tmp/app.log")).resolves.toEqual({
      path: "/tmp/app.log",
      bytes: 120,
      preview_bytes: 64,
      truncated: true,
      content: "hello",
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/preview",
    );
    expect(new URL(request.url).searchParams.get("path")).toBe("/tmp/app.log");
    expect(request.credentials).toBe("include");
  });

  it("deletes selected remote paths through a dedicated JSON endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          paths: ["/tmp/one.txt", "/tmp/two.txt"],
          count: 2,
          duration_ms: 9,
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(
      deleteSshSessionFiles("session-1", ["/tmp/one.txt", "/tmp/two.txt"]),
    ).resolves.toEqual({
      paths: ["/tmp/one.txt", "/tmp/two.txt"],
      count: 2,
      duration_ms: 9,
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/delete",
    );
    expect(request.method).toBe("POST");
    expect(request.credentials).toBe("include");
    expect(await request.clone().json()).toEqual({ paths: ["/tmp/one.txt", "/tmp/two.txt"] });
  });

  it("moves selected remote paths through a dedicated JSON endpoint", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          paths: ["/tmp/one.txt"],
          destination: "/tmp/archive",
          count: 1,
          duration_ms: 11,
        }),
        {
          status: 200,
          headers: { "content-type": "application/json" },
        },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(moveSshSessionFiles("session-1", ["/tmp/one.txt"], "/tmp/archive")).resolves.toEqual({
      paths: ["/tmp/one.txt"],
      destination: "/tmp/archive",
      count: 1,
      duration_ms: 11,
    });
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(new URL(request.url).pathname).toBe(
      "/api/v1/connections/sessions/session-1/files/move",
    );
    expect(request.method).toBe("POST");
    expect(request.credentials).toBe("include");
    expect(await request.clone().json()).toEqual({
      paths: ["/tmp/one.txt"],
      destination: "/tmp/archive",
    });
  });

  it("starts downloads by handing an anchor to the browser", () => {
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function click(
      this: HTMLAnchorElement,
    ) {
      clicked.push(this);
    });

    startSshSessionFileDownload("session-1", "/tmp/report.txt", "report.txt");

    const anchor = clicked[0];
    if (!anchor) throw new Error("download anchor was not clicked");
    expect(anchor.getAttribute("href")).toBe(
      "/api/v1/connections/sessions/session-1/files/download?path=%2Ftmp%2Freport.txt",
    );
    expect(anchor.getAttribute("download")).toBe("report.txt");
    expect(document.body.contains(anchor)).toBe(false);
  });

  it("starts archive downloads with archive query", () => {
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function click(
      this: HTMLAnchorElement,
    ) {
      clicked.push(this);
    });

    startSshSessionFileDownload("session-1", "/tmp/reports", "reports.tar.gz", {
      archive: true,
    });

    const anchor = clicked[0];
    if (!anchor) throw new Error("download anchor was not clicked");
    expect(anchor.getAttribute("href")).toBe(
      "/api/v1/connections/sessions/session-1/files/download?path=%2Ftmp%2Freports&archive=true",
    );
    expect(anchor.getAttribute("download")).toBe("reports.tar.gz");
  });

  it("uploads through XHR with progress and remote-copy notification", async () => {
    vi.stubGlobal("XMLHttpRequest", FakeXMLHttpRequest);
    const onProgress = vi.fn();
    const onRemoteCopy = vi.fn();

    await expect(
      uploadSshSessionFile("session-1", "/tmp/file.txt", new File(["0123456789"], "file.txt"), {
        onProgress,
        onRemoteCopy,
      }),
    ).resolves.toEqual({ path: "/tmp/file.txt", bytes: 10, duration_ms: 5 });

    const xhr = FakeXMLHttpRequest.instances[0];
    if (!xhr) throw new Error("XHR was not created");
    expect(xhr.method).toBe("POST");
    expect(new URL(xhr.url).pathname).toBe("/api/v1/connections/sessions/session-1/files/upload");
    expect(new URL(xhr.url).searchParams.get("path")).toBe("/tmp/file.txt");
    expect(xhr.withCredentials).toBe(true);
    expect(xhr.headers.has("x-request-id")).toBe(true);
    expect(onProgress).toHaveBeenCalledWith({ loaded: 10, total: 10, percent: 100 });
    expect(onRemoteCopy).toHaveBeenCalledTimes(1);
  });

  it("renews once and resends the same multipart file after an upload401", async () => {
    const fetchMock = vi.fn().mockResolvedValue(Response.json({ status: "refreshed" }));
    vi.stubGlobal("fetch", fetchMock);
    const forms: FormData[] = [];
    class ExpiringUpload extends FakeXMLHttpRequest {
      override send(form?: FormData) {
        if (!form) throw new Error("Missing upload body");
        forms.push(form);
        if (forms.length === 1) { this.status = 401; this.responseText = "{}"; this.onload?.(); }
        else super.send(form);
      }
    }
    vi.stubGlobal("XMLHttpRequest", ExpiringUpload);
    const file = new File(["0123456789"], "file.txt");
    const onRemoteCopy = vi.fn();
    await expect(uploadSshSessionFile("session-1", "/tmp/file.txt", file, { onRemoteCopy })).resolves.toMatchObject({ bytes: 10 });
    expect(forms).toHaveLength(2);
    expect(forms[0]).toBe(forms[1]);
    expect(forms[0]?.get("file")).toMatchObject({ name: "file.txt", size: 10 });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(new URL((fetchMock.mock.calls[0]?.[0] as Request).url).pathname).toBe("/api/v1/auth/session-refresh");
    expect(onRemoteCopy).toHaveBeenCalledTimes(1);
  });
});
