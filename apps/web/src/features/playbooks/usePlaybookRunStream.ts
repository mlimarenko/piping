/**
 * Live playbook-run event stream over the backend WebSocket
 * (`GET /api/v1/ws/playbooks/runs/{run_id}`).
 *
 * The backend `ws_playbook_run` handler **replays** the run's persisted events
 * (ordered by `event_sequence`) on connect, then **tails** live frames from the
 * in-process `RunEventBroadcaster`, and finally pushes a terminal `completion`
 * frame when the run reaches a terminal status. We reuse the reconnecting
 * `openSessionSocket` (cookie-authed, same-origin) with a 60s wall-clock
 * deadline; on give-up the caller falls back to REST polling.
 *
 * De-dup is by `event_sequence` (the one field both the WS frame and the REST
 * `PlaybookRunEvent` carry), so replay-after-reconnect never double-renders.
 */
import { useEffect, useRef, useState } from "react";

import {
  openSessionSocket,
  RECONNECT_GIVE_UP_REASON,
} from "@/features/connections/adapters/ws/sessionSocket";
import {
  playbookRunStreamUrl,
  type PlaybookRunEvent,
  type PlaybookRunEventKind,
  type PlaybookRunStatus,
} from "@/features/playbooks/api";

/** Tagged-union frame as serialised by `RunStreamFrame` on the backend. */
type RunStreamFrame =
  | {
      type: "event";
      run_group_id: string;
      run_id: string;
      host: string | null;
      event_sequence: number;
      event_kind: PlaybookRunEventKind;
      event_type: string;
      message: string | null;
      payload: unknown;
      created_at: string;
    }
  | {
      type: "completion";
      run_id: string;
      status: PlaybookRunStatus;
      exit_code: number | null;
      error_message: string | null;
    };

export interface RunStreamState {
  /** Events accumulated from the stream, ordered by `event_sequence`. */
  events: PlaybookRunEvent[];
  /** Terminal status from the completion frame (null while running). */
  status: PlaybookRunStatus | null;
  exitCode: number | null;
  errorMessage: string | null;
  /** The socket is currently OPEN (the run is being tailed live). */
  isLive: boolean;
  /** Reconnect deadline exceeded — the caller should poll instead. */
  gaveUp: boolean;
}

/**
 * Subscribe to a run's live event stream while `enabled`. Returns the
 * accumulated events plus the terminal status once the run completes. When the
 * socket can't be established within the reconnect deadline, `gaveUp` flips and
 * `isLive` stays false so the caller can degrade to polling.
 */
export function usePlaybookRunStream(
  runId: string,
  enabled: boolean,
): RunStreamState {
  const [events, setEvents] = useState<PlaybookRunEvent[]>([]);
  const [status, setStatus] = useState<PlaybookRunStatus | null>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [isLive, setIsLive] = useState(false);
  const [gaveUp, setGaveUp] = useState(false);

  // Keyed by event_sequence so replay-after-reconnect overwrites rather than
  // duplicates. A ref (not state) avoids re-subscribing on every frame.
  // The hook is mounted fresh per run (callers key the panel on run id), so the
  // accumulator and state start empty — no synchronous in-effect reset needed.
  const bySequence = useRef<Map<number, PlaybookRunEvent>>(new Map());

  useEffect(() => {
    if (!enabled || !runId) {
      return;
    }

    const controller = new AbortController();
    const socket = openSessionSocket({
      url: playbookRunStreamUrl(runId),
      reconnect: { baseDelayMs: 500, maxDelayMs: 5_000, deadlineMs: 60_000 },
      signal: controller.signal,
    });

    const offOpen = socket.addEventListener("open", () => {
      setIsLive(true);
    });
    const offError = socket.addEventListener("error", () => {
      setIsLive(false);
    });
    const offClose = socket.addEventListener("close", (event) => {
      setIsLive(false);
      if ((event as CloseEvent).reason === RECONNECT_GIVE_UP_REASON) {
        setGaveUp(true);
      }
    });
    const offMessage = socket.addEventListener("message", (event) => {
      const data = (event as MessageEvent<unknown>).data;
      if (typeof data !== "string") return;
      let parsed: unknown;
      try {
        parsed = JSON.parse(data);
      } catch {
        return;
      }
      const frame = parsed as RunStreamFrame;
      if (frame.type === "event") {
        const mapped: PlaybookRunEvent = {
          // Synthetic stable id — the list renders keyed on event_sequence.
          id: `${frame.run_id}:${frame.event_sequence}`,
          run_group_id: frame.run_group_id,
          run_id: frame.run_id,
          host: frame.host,
          event_sequence: frame.event_sequence,
          event_kind: frame.event_kind,
          event_type: frame.event_type,
          message: frame.message,
          payload: frame.payload,
          created_at: frame.created_at,
        };
        bySequence.current.set(frame.event_sequence, mapped);
        setEvents(
          Array.from(bySequence.current.values()).sort(
            (a, b) => a.event_sequence - b.event_sequence,
          ),
        );
      } else if (frame.type === "completion") {
        setStatus(frame.status);
        setExitCode(frame.exit_code);
        setErrorMessage(frame.error_message);
      }
    });

    return () => {
      offOpen();
      offError();
      offClose();
      offMessage();
      controller.abort();
      socket.close(1000, "unmount");
    };
  }, [runId, enabled]);

  return { events, status, exitCode, errorMessage, isLive, gaveUp };
}
