import { type PlaybookRunEvent } from "@/features/playbooks/api";
import {
  filterMessageForSelectedHost,
  isRawConsoleEvent,
  isRawRunnerStatusMessage,
  type AnsiSegment,
} from "@/features/playbooks/runOutput";

// ---------------------------------------------------------------------------
// Event text — colourises output using ansible event_type or message heuristics
// ---------------------------------------------------------------------------

// Fidelity contract: raw `console_line` events carry `ansible-playbook`'s own
// stdout/stderr verbatim. We never inject glyph prefixes (Ansible does not print
// them) and never recolour those lines — the console view must read 1:1 with a
// real terminal run. Ansible-native semantic colouring (green ok / red fatal /
// header separators) is applied ONLY to the structured-projection fallback used
// for legacy runs that predate the raw passthrough stream.
export function classifyEvent(event: PlaybookRunEvent): {
  colorClass: string;
  prefix: string;
  isHeader: boolean;
} {
  if (isRawConsoleEvent(event)) {
    return { colorClass: "text-foreground/90", prefix: "", isHeader: false };
  }

  const et = event.event_type ?? "";
  const msg = event.message ?? "";

  if (et === "playbook_on_play_start" || msg.startsWith("PLAY [") || msg.startsWith("PLAY *")) {
    return { colorClass: "text-foreground/80 font-medium", prefix: "", isHeader: true };
  }
  if (et === "playbook_on_task_start" || msg.startsWith("TASK [")) {
    return { colorClass: "text-status-warning/80", prefix: "", isHeader: false };
  }
  if (et === "runner_ok" || /^\s*ok:/.test(msg)) {
    return { colorClass: "text-status-positive", prefix: "", isHeader: false };
  }
  if (et === "runner_changed" || /^\s*changed:/.test(msg)) {
    return { colorClass: "text-status-warning", prefix: "", isHeader: false };
  }
  if (
    et === "runner_failed" ||
    et === "runner_unreachable" ||
    event.event_kind === "stderr" ||
    /^\s*(fatal|UNREACHABLE|ERROR):/.test(msg)
  ) {
    return { colorClass: "text-status-negative", prefix: "", isHeader: false };
  }
  if (et === "playbook_on_stats" || msg.startsWith("PLAY RECAP")) {
    return { colorClass: "text-foreground/70 font-medium", prefix: "", isHeader: true };
  }
  if (event.event_kind === "status") {
    return { colorClass: "text-status-warning/80", prefix: "", isHeader: false };
  }
  return { colorClass: "text-muted-foreground/70", prefix: "", isHeader: false };
}

export function scopedEventMessage(
  event: PlaybookRunEvent,
  selectedHost?: string | null,
  selectedRunId?: string | null,
): string {
  const message = event.message ?? "";
  if (!selectedHost || event.host) return message;
  return filterMessageForSelectedHost(message, selectedHost, selectedRunId);
}

export function latestReadableStatusMessage(
  events: PlaybookRunEvent[],
  selectedHost?: string | null,
  selectedRunId?: string | null,
): string | null {
  for (let i = events.length - 1; i >= 0; i -= 1) {
    const event = events[i];
    if (!event || event.event_kind !== "status" || !event.message) continue;
    if (isRawRunnerStatusMessage(event.message)) continue;
    return filterMessageForSelectedHost(event.message, selectedHost ?? null, selectedRunId);
  }
  return null;
}

/** Key each ANSI segment by its cumulative character offset (content-derived,
 *  stable across renders) rather than the array index, which the reconciler
 *  treats as positional. */
export function keySegmentsByOffset(
  segments: AnsiSegment[],
): { key: string; segment: AnsiSegment }[] {
  const keyed: { key: string; segment: AnsiSegment }[] = [];
  let offset = 0;
  for (const segment of segments) {
    keyed.push({ key: `${offset}:${segment.text}`, segment });
    offset += segment.text.length;
  }
  return keyed;
}
