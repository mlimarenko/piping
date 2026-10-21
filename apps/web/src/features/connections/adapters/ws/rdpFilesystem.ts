/**
 * RDP session filesystem, backed by Guacamole RDPDR drive redirection.
 *
 * When the backend launches an RDP session with `enable-drive` (see
 * `apps/api/src/proxy/rdp.rs`), guacd exposes a virtual disk inside the Windows
 * session and emits a `filesystem` instruction. The browser receives a
 * `Guacamole.Object` whose streams let us browse that disk, download files, and
 * upload files — the RDP analogue of the SSH SFTP file browser.
 *
 * Capability note: the Guacamole filesystem object is read/write only. Listing,
 * download, and upload are supported over the protocol; deleting, moving, and
 * creating directories are NOT part of the object stream API and must be done
 * inside the remote session itself. This matches Apache Guacamole's own RDP
 * drive behaviour.
 */

import Guacamole from "guacamole-common-js";

/** A single entry in a redirected-drive directory listing. */
export interface RdpFsEntry {
  /** Display name (last path segment). */
  name: string;
  /** Full stream name / absolute path within the drive (e.g. `/folder/file.txt`). */
  path: string;
  /** Mimetype reported by guacd. Directories use `STREAM_INDEX_MIMETYPE`. */
  mimetype: string;
  /** True when this entry is a directory (recursable via `list`). */
  isDirectory: boolean;
}

export interface RdpFilesystem {
  /** Drive label reported by guacd (e.g. `Piping`). */
  readonly name: string;
  /** Lists a directory. Defaults to the drive root. */
  list(path?: string): Promise<RdpFsEntry[]>;
  /** Downloads a file entry's contents as a Blob. */
  download(entry: RdpFsEntry): Promise<Blob>;
  /**
   * Uploads a local File into `dirPath`, keeping the file's own name.
   * `onProgress` reports bytes written so far against `file.size`.
   */
  upload(
    dirPath: string,
    file: File,
    onProgress?: (loaded: number, total: number) => void,
  ): Promise<void>;
}

const INDEX_MIMETYPE = Guacamole.Object.STREAM_INDEX_MIMETYPE;
const ROOT_STREAM = Guacamole.Object.ROOT_STREAM;
const BODY_ARRIVAL_TIMEOUT_MS = 20_000;
const BODY_INACTIVITY_TIMEOUT_MS = 20_000;
const STALE_BODY_GRACE_MS = 2_000;
const UPLOAD_TIMEOUT_MS = 20_000;
const UPLOAD_CHUNK_BYTES = Guacamole.ArrayBufferWriter.DEFAULT_BLOB_LENGTH;

type BodyHandler = (
  stream: Guacamole.InputStream,
  mimetype: string,
  accept: () => void,
  resolve: (value: unknown) => void,
  reject: (error: Error) => void,
) => void;

interface PendingBodyRequest {
  name: string;
  timeoutMessage: string;
  timer?: ReturnType<typeof setTimeout>;
  wireIssued: boolean;
  bodyReceived: boolean;
  settled: boolean;
  handleBody: BodyHandler;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
}

interface BodyRequestState {
  active?: PendingBodyRequest;
  queue: PendingBodyRequest[];
  reconcileTimer?: ReturnType<typeof setTimeout>;
}

type RoutedGuacamoleObject = Omit<Guacamole.Object, "onbody"> & {
  onbody: ((stream: Guacamole.InputStream, mimetype: string, name: string) => void) | null;
};

function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const slash = trimmed.lastIndexOf("/");
  return slash >= 0 ? trimmed.slice(slash + 1) : trimmed;
}

/** Joins a directory path and a file name into a single absolute drive path. */
export function joinDrivePath(dir: string, name: string): string {
  const base = dir === ROOT_STREAM ? "" : dir.replace(/\/+$/, "");
  return `${base}/${name}`;
}

function toError(value: unknown, fallback: string): Error {
  return value instanceof Error ? value : new Error(fallback);
}

function acceptInputStream(stream: Guacamole.InputStream): void {
  stream.sendAck("OK", 0);
}

function drainInputStream(stream: Guacamole.InputStream, onEnd?: () => void): void {
  stream.onblob = () => {
    stream.sendAck("OK", 0);
  };
  stream.onend = () => {
    onEnd?.();
  };
}

function startInactivityTimer(message: string, reject: (error: Error) => void) {
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clear = () => {
    if (timer) clearTimeout(timer);
    timer = null;
  };

  const reset = () => {
    clear();
    timer = setTimeout(() => {
      reject(new Error(message));
    }, BODY_INACTIVITY_TIMEOUT_MS);
  };

  reset();
  return { clear, reset };
}

/**
 * Wraps a `Guacamole.Object` (a redirected RDP drive) in a Promise-based
 * filesystem facade. Guacd waits for an ACK after a `body` instruction before
 * sending stream blobs. Binary reader/writer helpers from guacamole-common-js
 * acknowledge blobs internally. Directory index reads use StringReader, whose
 * ArrayBufferReader base does not ack blobs, so list() acknowledges text chunks
 * explicitly.
 */
export function createRdpFilesystem(driveObject: Guacamole.Object, name: string): RdpFilesystem {
  const routedDrive = driveObject as unknown as RoutedGuacamoleObject;
  const requestStates = new Map<string, BodyRequestState>();

  function stateFor(streamName: string): BodyRequestState {
    let state = requestStates.get(streamName);
    if (!state) {
      state = { queue: [] };
      requestStates.set(streamName, state);
    }
    return state;
  }

  function cleanupState(streamName: string): void {
    const state = requestStates.get(streamName);
    if (!state) return;
    if (!state.active && state.queue.length === 0 && !state.reconcileTimer) {
      requestStates.delete(streamName);
    }
  }

  function finishReconcile(streamName: string, state: BodyRequestState): void {
    if (state.reconcileTimer) {
      clearTimeout(state.reconcileTimer);
      state.reconcileTimer = undefined;
    }
    dispatchNext(streamName);
    cleanupState(streamName);
  }

  function startReconcile(streamName: string, state: BodyRequestState): void {
    if (state.reconcileTimer) return;

    state.reconcileTimer = setTimeout(() => {
      const current = requestStates.get(streamName);
      if (!current) return;
      current.reconcileTimer = undefined;
      finishReconcile(streamName, current);
    }, STALE_BODY_GRACE_MS);
  }

  function removeQueuedRequest(state: BodyRequestState, request: PendingBodyRequest): void {
    const index = state.queue.indexOf(request);
    if (index >= 0) state.queue.splice(index, 1);
  }

  function clearRequestTimer(request: PendingBodyRequest): void {
    if (request.timer) clearTimeout(request.timer);
    request.timer = undefined;
  }

  function startRequestTimer(request: PendingBodyRequest): void {
    clearRequestTimer(request);
    request.timer = setTimeout(() => timeoutRequest(request), BODY_ARRIVAL_TIMEOUT_MS);
  }

  function dispatchNext(streamName: string): void {
    const state = requestStates.get(streamName);
    if (!state || state.active || state.reconcileTimer) return;

    const next = state.queue.shift();
    if (!next) {
      cleanupState(streamName);
      return;
    }

    state.active = next;
    next.wireIssued = true;
    startRequestTimer(next);
    driveObject.requestInputStream(streamName);
  }

  function settleRequest(
    request: PendingBodyRequest,
    outcome: { ok: true; value: unknown } | { ok: false; error: Error },
  ): void {
    if (request.settled) return;
    request.settled = true;
    clearRequestTimer(request);

    const state = requestStates.get(request.name);
    if (state?.active === request) {
      state.active = undefined;
    } else if (state) {
      removeQueuedRequest(state, request);
    }

    if (outcome.ok) {
      request.resolve(outcome.value);
    } else {
      request.reject(outcome.error);
    }

    dispatchNext(request.name);
    cleanupState(request.name);
  }

  function timeoutRequest(request: PendingBodyRequest): void {
    if (request.settled) return;
    request.settled = true;
    clearRequestTimer(request);

    const state = requestStates.get(request.name);
    if (state?.active === request) {
      state.active = undefined;
      if (request.wireIssued && !request.bodyReceived) {
        startReconcile(request.name, state);
      }
    } else if (state) {
      removeQueuedRequest(state, request);
    }

    request.reject(new Error(request.timeoutMessage));
    dispatchNext(request.name);
    cleanupState(request.name);
  }

  function requestBody<T>(
    streamName: string,
    timeoutMessage: string,
    handleBody: (
      stream: Guacamole.InputStream,
      mimetype: string,
      accept: () => void,
      resolve: (value: T) => void,
      reject: (error: Error) => void,
    ) => void,
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const request: PendingBodyRequest = {
        name: streamName,
        timeoutMessage,
        wireIssued: false,
        bodyReceived: false,
        settled: false,
        handleBody,
        resolve: (value) => resolve(value as T),
        reject,
      };

      stateFor(streamName).queue.push(request);
      dispatchNext(streamName);
    });
  }

  routedDrive.onbody = (stream, mimetype, streamName) => {
    const state = requestStates.get(streamName);
    if (state?.reconcileTimer) {
      drainInputStream(stream, () => {
        finishReconcile(streamName, state);
      });
      acceptInputStream(stream);
      return;
    }

    const request = state?.active;
    if (!request) {
      drainInputStream(stream);
      acceptInputStream(stream);
      return;
    }

    request.bodyReceived = true;
    clearRequestTimer(request);
    const accept = () => acceptInputStream(stream);
    request.handleBody(
      stream,
      mimetype,
      accept,
      (value) => settleRequest(request, { ok: true, value }),
      (error) => settleRequest(request, { ok: false, error }),
    );
  };

  return {
    name,

    list(path = ROOT_STREAM): Promise<RdpFsEntry[]> {
      return requestBody<RdpFsEntry[]>(
        path,
        `Directory listing timed out: ${path}`,
        (stream, mimetype, accept, resolve, reject) => {
          if (mimetype !== INDEX_MIMETYPE) {
            drainInputStream(stream);
            accept();
            reject(new Error(`Not a directory: ${path}`));
            return;
          }
          const inactivity = startInactivityTimer(
            `Directory listing timed out: ${path}`,
            reject,
          );
          const reader = new Guacamole.StringReader(stream);
          let json = "";
          reader.ontext = (text) => {
            inactivity.reset();
            json += text;
            stream.sendAck("OK", 0);
          };
          reader.onend = () => {
            inactivity.clear();
            try {
              const indexMap = JSON.parse(json) as Record<string, string>;
              const entries = globalThis.Object.entries(indexMap).map<RdpFsEntry>(
                ([entryPath, entryMime]) => ({
                  name: basename(entryPath),
                  path: entryPath,
                  mimetype: entryMime,
                  isDirectory: entryMime === INDEX_MIMETYPE,
                }),
              );
              entries.sort((a, b) =>
                a.isDirectory === b.isDirectory
                  ? a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
                  : a.isDirectory
                  ? -1
                  : 1,
              );
              resolve(entries);
            } catch (error) {
              reject(toError(error, "Failed to parse directory index"));
            }
          };
          accept();
        },
      );
    },

    download(entry: RdpFsEntry): Promise<Blob> {
      return requestBody<Blob>(
        entry.path,
        `Download timed out: ${entry.path}`,
        (stream, mimetype, accept, resolve, reject) => {
          const inactivity = startInactivityTimer(
            `Download timed out: ${entry.path}`,
            reject,
          );
          const reader = new Guacamole.BlobReader(
            stream,
            mimetype || entry.mimetype || "application/octet-stream",
          );
          reader.onprogress = () => {
            inactivity.reset();
          };
          reader.onend = () => {
            inactivity.clear();
            resolve(reader.getBlob());
          };
          accept();
        },
      );
    },

    upload(
      dirPath: string,
      file: File,
      onProgress?: (loaded: number, total: number) => void,
    ): Promise<void> {
      return new Promise<void>((resolve, reject) => {
        let settled = false;
        let lastLoaded = 0;
        let timer: ReturnType<typeof setTimeout> | null = null;
        const target = joinDrivePath(dirPath, file.name);
        const stream = driveObject.createOutputStream(
          file.type || "application/octet-stream",
          target,
        );
        const writer = new Guacamole.BlobWriter(stream);

        const clearTimer = () => {
          if (timer) clearTimeout(timer);
          timer = null;
        };
        const fail = (error: Error) => {
          if (settled) return;
          settled = true;
          clearTimer();
          reject(error);
        };
        const resetTimer = () => {
          clearTimer();
          timer = setTimeout(() => {
            fail(new Error(`Upload timed out: ${target}`));
          }, UPLOAD_TIMEOUT_MS);
        };

        writer.onack = (status) => {
          if (settled) return;
          if (status.isError()) {
            fail(new Error(status.message ? `Upload failed: ${status.message}` : "Upload failed"));
            return;
          }
          resetTimer();
        };
        writer.onprogress = (_blob, offset) => {
          resetTimer();
          lastLoaded = Math.max(lastLoaded, Math.min(file.size, offset + UPLOAD_CHUNK_BYTES));
          onProgress?.(lastLoaded, file.size);
        };
        writer.oncomplete = () => {
          if (settled) return;
          settled = true;
          clearTimer();
          writer.sendEnd();
          onProgress?.(file.size, file.size);
          resolve();
        };
        writer.onerror = (_blob, _offset, error) => {
          fail(toError(error, "Upload failed"));
        };
        resetTimer();
        writer.sendBlob(file);
      });
    },
  };
}
