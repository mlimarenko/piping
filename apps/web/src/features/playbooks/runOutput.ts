import type { PlaybookRunEvent } from "@/features/playbooks/api";

export interface AnsiSegment {
  text: string;
  className: string;
}

// Ansible's SGR palette mapped to theme tokens (base 16; 256/truecolor fall back
// to the default foreground). A FIXED className map is what keeps rendering
// XSS-safe with untrusted playbook output — no inline styles from the wire.
const ANSI_FG_CLASS: Record<number, string> = {
  30: "text-foreground/50",
  90: "text-foreground/50",
  31: "text-status-negative",
  91: "text-status-negative",
  32: "text-status-positive",
  92: "text-status-positive",
  33: "text-status-warning",
  93: "text-status-warning",
  34: "text-sky-400",
  94: "text-sky-400",
  35: "text-fuchsia-400",
  95: "text-fuchsia-400",
  36: "text-cyan-400",
  96: "text-cyan-400",
  37: "text-foreground/90",
  97: "text-foreground",
};

// Matches one CSI sequence: ESC '[' params intermediates final-byte.
// eslint-disable-next-line no-control-regex -- ANSI CSI parsing must match the literal ESC (0x1B) control character.
const ANSI_CSI = /\x1b\[[0-9;:?]*[ -/]*[@-~]/g;
// Same, but captured so the SGR parser can read the params + final byte.
// eslint-disable-next-line no-control-regex -- ANSI CSI parsing must match the literal ESC (0x1B) control character.
const ANSI_CSI_CAPTURED = /\x1b\[([0-9;:?]*)[ -/]*([@-~])/g;

/** Strip ANSI/CSI escape sequences so host-token matching runs on clean text.
 *  No-op (returns the input) when there is nothing to strip. */
export function stripAnsi(text: string): string {
  if (!text.includes("\x1b")) return text;
  return text.replace(ANSI_CSI, "");
}

interface SgrState {
  fg: string;
  bold: boolean;
}

// 256-colour (38;5;n) / truecolour (38;2;r;g;b): how many following params the
// extended-colour introducer consumes before we fall back to the default fg.
function extendedColorParamSkip(next: number | undefined): number {
  if (next === 5) return 2;
  if (next === 2) return 4;
  return 0;
}

// Apply one SGR sequence's codes to the running colour/bold state.
function applySgrCodes(state: SgrState, codes: number[]): SgrState {
  let { fg, bold } = state;
  for (let i = 0; i < codes.length; i += 1) {
    const code = codes[i];
    if (code === undefined) {
      continue;
    }
    if (code === 0) {
      fg = "";
      bold = false;
    } else if (code === 1) {
      bold = true;
    } else if (code === 22) {
      bold = false;
    } else if (code === 39) {
      fg = "";
    } else if (code === 38) {
      i += extendedColorParamSkip(codes[i + 1]);
      fg = "";
    } else {
      fg = ANSI_FG_CLASS[code] ?? fg;
    }
  }
  return { fg, bold };
}

/** Parse a line's ANSI SGR colouring into styled segments for rendering. Plain
 *  text returns a single unstyled segment; non-SGR CSI sequences are dropped. */
export function parseAnsiLine(text: string): AnsiSegment[] {
  if (!text.includes("\x1b")) return [{ text, className: "" }];
  const segments: AnsiSegment[] = [];
  let state: SgrState = { fg: "", bold: false };
  const pushText = (chunk: string) => {
    if (!chunk) return;
    const className = [state.bold ? "font-medium" : "", state.fg].filter(Boolean).join(" ");
    const previous = segments.at(-1);
    // Merge adjacent same-style runs so a plain line (or a non-SGR CSI split) is
    // one segment, not many — fewer DOM nodes and a stable single-segment shape.
    if (previous?.className === className) {
      previous.text += chunk;
    } else {
      segments.push({ text: chunk, className });
    }
  };
  ANSI_CSI_CAPTURED.lastIndex = 0;
  let last = 0;
  let match: RegExpExecArray | null;
  while ((match = ANSI_CSI_CAPTURED.exec(text)) !== null) {
    pushText(text.slice(last, match.index));
    last = ANSI_CSI_CAPTURED.lastIndex;
    if (match[2] !== "m") continue; // non-SGR CSI: consumed, not rendered
    const params = match[1] ?? "";
    const codes = params === "" ? [0] : params.split(";").map((part) => Number(part) || 0);
    state = applySgrCodes(state, codes);
  }
  pushText(text.slice(last));
  // If the line was ALL escape sequences with no visible text (e.g. ansible's
  // empty coloured separator `\x1b[0;31m\x1b[0m`), no chunk was ever pushed.
  // Return an EMPTY segment — never the raw input — so the caller renders an
  // empty line instead of leaking literal `[0;31m` codes.
  return segments.length > 0 ? segments : [{ text: "", className: "" }];
}

export function normalizeAnsibleHostToken(value: string): string {
  return value.trim().toLocaleLowerCase().replace(/\s+/g, "_");
}

export function ansibleRunAliasSuffix(runId: string | null | undefined): string | null {
  const normalized = runId?.replace(/-/gu, "").toLocaleLowerCase() ?? "";
  if (!/^[0-9a-f]{32}$/u.test(normalized)) return null;
  return normalized.slice(-16);
}

function ansibleRunAliasCandidates(runId: string | null | undefined): string[] {
  const normalized = runId?.replaceAll("-", "").toLocaleLowerCase() ?? "";
  if (!/^[0-9a-f]{32}$/u.test(normalized)) return [];
  return Array.from(new Set([normalized.slice(-16), normalized.slice(0, 8)]));
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function payloadRecord(event: PlaybookRunEvent): Record<string, unknown> | null {
  return asRecord(event.payload);
}

export function isRawConsoleEvent(event: PlaybookRunEvent): boolean {
  const payload = payloadRecord(event);
  return (
    event.event_type === "console_line" &&
    (event.event_kind === "stdout" || event.event_kind === "stderr") &&
    payload?.raw === true
  );
}

function isRunnerConsoleNotice(event: PlaybookRunEvent): boolean {
  return (
    event.event_type === "stdout" &&
    event.event_kind === "stdout" &&
    payloadRecord(event)?.stream === "runner"
  );
}

function isConsoleOutputEvent(event: PlaybookRunEvent): boolean {
  return (
    event.event_kind === "stdout" ||
    event.event_kind === "stderr" ||
    event.event_kind === "result"
  );
}

export function consoleOutputEvents(events: PlaybookRunEvent[]): PlaybookRunEvent[] {
  const hasRawConsole = events.some(isRawConsoleEvent);
  if (!hasRawConsole) return events.filter(isConsoleOutputEvent);
  return events.filter((event) => isRawConsoleEvent(event) || isRunnerConsoleNotice(event));
}

function payloadResult(event: PlaybookRunEvent): Record<string, unknown> | null {
  const payload = payloadRecord(event);
  return payload ? asRecord(payload.res) : null;
}

function payloadTask(event: PlaybookRunEvent): string | null {
  const task = payloadRecord(event)?.task;
  return typeof task === "string" && task.trim() ? task.trim() : null;
}

function resultMessage(event: PlaybookRunEvent): unknown {
  const result = payloadResult(event);
  if (!result) return null;
  return result.msg ?? result;
}

function isNoisyRestResult(message: string): boolean {
  return (
    message.includes("'cookies_string':") ||
    message.includes('"cookies_string":') ||
    message.includes("'access-control-allow-credentials':") ||
    message.includes('"access-control-allow-credentials":') ||
    message.includes("'x-request-id':") ||
    message.includes('"x-request-id":') ||
    message.includes("'content_type': 'application/json'") ||
    message.includes('"content_type": "application/json"')
  );
}

function ansibleResultPrefix(message: string): string | null {
  const match = message.match(/^\s*(ok|changed|failed|fatal):\s+\[[^\]]+\]/u);
  return match?.[0] ?? null;
}

function inlineResultSummary(message: string): string | null {
  const parts: string[] = [];
  const patterns: Array<[string, RegExp]> = [
    ["changed", /['"]?changed['"]?\s*:\s*(True|False|true|false)/u],
    ["failed", /['"]?failed['"]?\s*:\s*(True|False|true|false)/u],
    ["skipped", /['"]?skipped['"]?\s*:\s*(True|False|true|false)/u],
    ["status", /['"]?status['"]?\s*:\s*([0-9]+)/u],
    ["rc", /['"]?rc['"]?\s*:\s*(-?[0-9]+)/u],
  ];

  for (const [key, pattern] of patterns) {
    const match = message.match(pattern);
    if (!match?.[1]) continue;
    parts.push(`${key}: ${match[1].toLocaleLowerCase()}`);
  }

  return parts.length > 0 ? parts.join(", ") : null;
}

function formatResultObjectMessage(value: unknown): string | null {
  if (typeof value === "string") return value.trim() || null;
  const record = asRecord(value);
  if (!record) return null;

  const simplePairs = Object.entries(record)
    .filter(([, item]) => ["string", "number", "boolean"].includes(typeof item) || item === null)
    .slice(0, 8)
    .map(([key, item]) => `${key}: ${String(item)}`);
  return simplePairs.length > 0 ? simplePairs.join(", ") : null;
}

function collapseVerboseResult(event: PlaybookRunEvent, message: string): string | null {
  if (event.event_type.includes("skipped") || /^\s*skipping:/u.test(message)) {
    return null;
  }

  const prefix = ansibleResultPrefix(message);
  if (!prefix) return message;

  const msg = formatResultObjectMessage(resultMessage(event));
  if (msg) return `${prefix} => ${msg}`;

  if (
    message.length > 500 ||
    isNoisyRestResult(message) ||
    /['"]warnings['"]\s*:/u.test(message) ||
    /['"]ansible_facts['"]\s*:/u.test(message)
  ) {
    const inlineSummary = inlineResultSummary(message);
    if (inlineSummary) return `${prefix} => ${inlineSummary}`;

    const task = payloadTask(event);
    return task ? `${prefix} · ${task}` : prefix;
  }

  return message;
}

function stripAnsibleBannerNoise(message: string): string {
  return message
    .split(/\r?\n/u)
    .map((line) => line.replace(/\s+\*{8,}\s*$/u, "").trimEnd())
    .filter((line) => !/^\*{8,}\s*$/u.test(line.trim()))
    .join("\n")
    .trimEnd();
}

function isAnsibleHostTokenCharacter(value: string): boolean {
  return /^[\p{L}\p{N}_-]$/u.test(value);
}

export function lineContainsAnsibleHostToken(line: string, hostToken: string): boolean {
  let fromIndex = 0;
  while (fromIndex < line.length) {
    const index = line.indexOf(hostToken, fromIndex);
    if (index < 0) return false;

    const before = index > 0 ? line[index - 1] : "";
    const after = line.slice(index + hostToken.length);
    const hasLeftBoundary = !before || !isAnsibleHostTokenCharacter(before);
    const hasRightBoundary =
      after.length === 0 ||
      !isAnsibleHostTokenCharacter(after[0] ?? "") ||
      /^_[0-9a-f]{8,}/u.test(after);

    if (hasLeftBoundary && hasRightBoundary) return true;
    fromIndex = index + hostToken.length;
  }
  return false;
}

function runHostAliasSuffixes(line: string): string[] {
  const normalizedLine = line.trim().toLocaleLowerCase();
  return Array.from(
    normalizedLine.matchAll(
      /[\p{L}\p{N}_-]+_([0-9a-f]{8,})(?=$|[^\p{L}\p{N}_-])/giu,
    ),
    (match) => match[1]?.toLocaleLowerCase() ?? "",
  ).filter(Boolean);
}

function lineContainsSelectedRunAlias(
  line: string,
  selectedRunId: string | null | undefined,
): boolean {
  const candidates = ansibleRunAliasCandidates(selectedRunId);
  if (candidates.length === 0) return false;
  return runHostAliasSuffixes(line).some((suffix) => candidates.includes(suffix));
}

export function messageLineMatchesSelectedHost(
  line: string,
  selectedHost: string,
  selectedRunId: string | null | undefined = null,
): boolean {
  if (lineContainsSelectedRunAlias(line, selectedRunId)) return true;
  const selectedToken = normalizeAnsibleHostToken(selectedHost);
  if (!selectedToken) return true;
  return lineContainsAnsibleHostToken(normalizeAnsibleHostToken(line), selectedToken);
}

function lineContainsOtherRunHostAlias(
  line: string,
  selectedRunId: string | null | undefined,
): boolean {
  const suffixes = runHostAliasSuffixes(line);
  if (suffixes.length === 0) return false;
  const selectedSuffixes = ansibleRunAliasCandidates(selectedRunId);
  return selectedSuffixes.length === 0 || suffixes.every((suffix) => !selectedSuffixes.includes(suffix));
}

function isAnsibleStructuralLine(line: string): boolean {
  const trimmed = line.trim();
  return (
    /^(PLAY|TASK|RUNNING HANDLER|PLAY RECAP)\b/u.test(trimmed) ||
    /^\*{8,}$/u.test(trimmed)
  );
}

export function filterMessageForSelectedHost(
  message: string,
  selectedHost: string | null,
  selectedRunId: string | null | undefined = null,
): string {
  if (!selectedHost && !selectedRunId) return message;
  const lines = message.split(/\r?\n/);
  const kept: string[] = [];
  let droppingOtherHostBlock = false;

  for (const line of lines) {
    // Match host/alias tokens against a colour-stripped copy so ANSI codes around
    // the token can't defeat detection; keep the ORIGINAL coloured line to render.
    const match = stripAnsi(line);
    const trimmed = match.trim();
    if (trimmed === "") {
      if (!droppingOtherHostBlock) kept.push(line);
      droppingOtherHostBlock = false;
      continue;
    }

    if (isAnsibleStructuralLine(match)) {
      droppingOtherHostBlock = false;
      kept.push(line);
      continue;
    }

    if (selectedHost && messageLineMatchesSelectedHost(match, selectedHost, selectedRunId)) {
      droppingOtherHostBlock = false;
      kept.push(line);
      continue;
    }

    if (!selectedHost && lineContainsSelectedRunAlias(match, selectedRunId)) {
      droppingOtherHostBlock = false;
      kept.push(line);
      continue;
    }

    if (lineContainsOtherRunHostAlias(match, selectedRunId)) {
      droppingOtherHostBlock = true;
      continue;
    }

    if (!droppingOtherHostBlock) kept.push(line);
  }

  return kept.join("\n").trimEnd();
}

export function formatRunOutputEvent(
  event: PlaybookRunEvent,
  selectedHost: string | null = null,
  selectedRunId: string | null | undefined = null,
): string | null {
  // A console line with no message is an empty line, never the event type name.
  // Falling back to `event.event_type` leaked literal "console_line" tokens into
  // the rendered Ansible output.
  const rawMessage = event.message ?? "";
  if (isRawConsoleEvent(event)) {
    if (!selectedHost && !selectedRunId) return rawMessage;
    const scoped = filterMessageForSelectedHost(rawMessage, selectedHost, selectedRunId);
    return scoped || null;
  }
  if (isRawRunnerStatusMessage(rawMessage)) return null;

  const scoped = stripAnsibleBannerNoise(
    filterMessageForSelectedHost(rawMessage, selectedHost, selectedRunId),
  );
  if (!scoped) return null;
  return collapseVerboseResult(event, scoped);
}

export function isRawRunnerStatusMessage(message: string): boolean {
  const normalized = message.trim();
  return (
    /^(runner_|playbook_)[a-z_]+\b/u.test(normalized) ||
    normalized === "Run started by worker lease owner" ||
    normalized === "Ansible job dispatched; waiting for runner events"
  );
}
