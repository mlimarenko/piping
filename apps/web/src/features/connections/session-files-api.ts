import { API_BASE_URL, ApiError, apiErrorFromResponse, fetchWithSession, sendWithSession } from "@/shared/api/client";

export interface SshFileEntry {
  name: string;
  path: string;
  kind: string;
  size?: number | null;
  modified_at?: string | null;
  permissions?: string | null;
}

export interface SshFileList {
  path: string;
  parent?: string | null;
  entries: SshFileEntry[];
}

export interface SshFileTransfer {
  path: string;
  bytes: number;
  duration_ms: number;
}

export interface SshFilePreview {
  path: string;
  bytes: number;
  preview_bytes: number;
  truncated: boolean;
  content: string;
}

export interface SshFileDeleteResult {
  paths: string[];
  count: number;
  duration_ms: number;
}

export interface SshFileMoveResult {
  paths: string[];
  destination: string;
  count: number;
  duration_ms: number;
}

export interface SshFileDownloadPreflight {
  path: string;
  file_name: string;
  bytes?: number | null;
  logical_bytes?: number | null;
  entry_count?: number | null;
  kind: string;
  archive: boolean;
}

export interface SshFileUploadProgress {
  loaded: number;
  total?: number;
  percent?: number;
}

export interface UploadSshSessionFileOptions {
  signal?: AbortSignal;
  onProgress?: (progress: SshFileUploadProgress) => void;
  onRemoteCopy?: () => void;
}

export interface SshFileDownloadOptions {
  archive?: boolean;
}

function requestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `req-${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 10)}`;
}

function sessionFilesUrl(
  sessionId: string,
  suffix: string,
  params?: Record<string, string | undefined>,
): string {
  const search = new URLSearchParams();
  Object.entries(params ?? {}).forEach(([key, value]) => {
    if (value !== undefined) search.set(key, value);
  });
  const query = search.size > 0 ? `?${search.toString()}` : "";
  return `${API_BASE_URL}/api/v1/connections/sessions/${encodeURIComponent(sessionId)}/files${suffix}${query}`;
}

async function readJson<T>(response: Response): Promise<T> {
  if (!response.ok) throw await apiErrorFromResponse(response);
  return (await response.json()) as T;
}

export async function listSshSessionFiles(sessionId: string, path = "."): Promise<SshFileList> {
  const response = await fetchWithSession(sessionFilesUrl(sessionId, "", { path }), {
    credentials: "include",
    headers: { "X-Request-Id": requestId() },
  });
  return readJson<SshFileList>(response);
}

export async function uploadSshSessionFile(
  sessionId: string,
  remotePath: string,
  file: File,
  options: UploadSshSessionFileOptions = {},
): Promise<SshFileTransfer> {
  const form = new FormData();
  form.append("file", file, file.name);
  const request = new Request(new URL(sessionFilesUrl(sessionId, "/upload", { path: remotePath }), window.location.href), {
    method: "POST", credentials: "include", signal: options.signal,
    headers: { "X-Request-Id": requestId() },
  });
  // FormData is reusable; XHR owns its multipart boundary and upload progress.
  let remoteCopyAnnounced = false;
  const response = await sendWithSession(request, (attempt) => new Promise<Response>((resolve, reject) => {

    const xhr = new XMLHttpRequest();
    const announceRemoteCopy = () => {
      if (remoteCopyAnnounced) return;
      remoteCopyAnnounced = true;
      options.onRemoteCopy?.();
    };
    const cleanup = () => {
      options.signal?.removeEventListener("abort", abort);
    };
    const abort = () => {
      xhr.abort();
    };

    xhr.open(attempt.method, attempt.url, true);
    xhr.withCredentials = true;
    attempt.headers.forEach((value, name) => xhr.setRequestHeader(name, value));
    xhr.responseType = "text";
    xhr.upload.onprogress = (event) => {
      const total = event.lengthComputable ? event.total : undefined;
      options.onProgress?.({
        loaded: event.loaded,
        total,
        percent: total ? Math.min(100, Math.round((event.loaded / total) * 100)) : undefined,
      });
      if (total !== undefined && event.loaded >= total) {
        announceRemoteCopy();
      }
    };
    xhr.upload.onload = announceRemoteCopy;
    xhr.onload = () => {
      cleanup();
      if (xhr.status === 0) { reject(new ApiError({ status: 0, message: "Upload request failed" })); return; }
      resolve(new Response(xhr.responseText, { status: xhr.status, headers: { "Content-Type": "application/json" } }));
    };
    xhr.onerror = () => {
      cleanup();
      reject(new ApiError({ status: 0, message: "Upload request failed" }));
    };
    xhr.onabort = () => {
      cleanup();
      reject(new DOMException("Upload cancelled", "AbortError"));
    };
    if (options.signal) {
      if (options.signal.aborted) {
        reject(new DOMException("Upload cancelled", "AbortError"));
        return;
      }
      options.signal.addEventListener("abort", abort, { once: true });
    }
    xhr.send(form);
  }));
  if (!response.ok) throw await apiErrorFromResponse(response);
  try { return await response.json() as SshFileTransfer; }
  catch { throw new ApiError({ status: response.status, message: "Invalid upload response" }); }
}

export async function ensureSshSessionDirectory(
  sessionId: string,
  remotePath: string,
): Promise<SshFileTransfer> {
  const response = await fetchWithSession(sessionFilesUrl(sessionId, "/mkdir", { path: remotePath }), {
    method: "POST",
    credentials: "include",
    headers: { "X-Request-Id": requestId() },
  });
  return readJson<SshFileTransfer>(response);
}

export async function previewSshSessionFile(
  sessionId: string,
  remotePath: string,
): Promise<SshFilePreview> {
  const response = await fetchWithSession(sessionFilesUrl(sessionId, "/preview", { path: remotePath }), {
    credentials: "include",
    headers: { "X-Request-Id": requestId() },
  });
  return readJson<SshFilePreview>(response);
}

export async function deleteSshSessionFiles(
  sessionId: string,
  remotePaths: string[],
): Promise<SshFileDeleteResult> {
  const response = await fetchWithSession(sessionFilesUrl(sessionId, "/delete"), {
    method: "POST",
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      "X-Request-Id": requestId(),
    },
    body: JSON.stringify({ paths: remotePaths }),
  });
  return readJson<SshFileDeleteResult>(response);
}

export async function moveSshSessionFiles(
  sessionId: string,
  remotePaths: string[],
  destination: string,
): Promise<SshFileMoveResult> {
  const response = await fetchWithSession(sessionFilesUrl(sessionId, "/move"), {
    method: "POST",
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      "X-Request-Id": requestId(),
    },
    body: JSON.stringify({ paths: remotePaths, destination }),
  });
  return readJson<SshFileMoveResult>(response);
}

export async function preflightSshSessionFileDownload(
  sessionId: string,
  remotePath: string,
  options: SshFileDownloadOptions = {},
): Promise<SshFileDownloadPreflight> {
  const response = await fetchWithSession(
    sessionFilesUrl(sessionId, "/download/preflight", {
      path: remotePath,
      archive: options.archive ? "true" : undefined,
    }),
    {
      credentials: "include",
      headers: { "X-Request-Id": requestId() },
    },
  );
  return readJson<SshFileDownloadPreflight>(response);
}

export function sshSessionFileDownloadUrl(
  sessionId: string,
  remotePath: string,
  options: SshFileDownloadOptions = {},
): string {
  return sessionFilesUrl(sessionId, "/download", {
    path: remotePath,
    archive: options.archive ? "true" : undefined,
  });
}

export function startSshSessionFileDownload(
  sessionId: string,
  remotePath: string,
  fileName?: string,
  options: SshFileDownloadOptions = {},
): void {
  const link = document.createElement("a");
  link.href = sshSessionFileDownloadUrl(sessionId, remotePath, options);
  if (fileName) {
    link.download = fileName;
  }
  link.rel = "noopener";
  document.body.append(link);
  link.click();
  link.remove();
}
