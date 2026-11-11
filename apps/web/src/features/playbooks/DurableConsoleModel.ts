export type ConsoleChunk = { sequence: number; stream: "stdout" | "stderr"; bytes: Uint8Array };
export type ConsolePage = { dispatchId: string | null; dispatchAttempt: number; chunks: ConsoleChunk[]; nextSequence: number; complete: boolean };

export class ConsoleConflict extends Error {}

const integer = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

/** Validate the whole untrusted page before any bytes are rendered. */
export function parseConsolePage(value: unknown): ConsolePage {
  if (!object(value) || !(value.dispatch_id === null || (typeof value.dispatch_id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value.dispatch_id) && value.dispatch_id !== "00000000-0000-0000-0000-000000000000")) || !integer(value.dispatch_attempt) || value.dispatch_attempt > 2147483647 || !integer(value.next_sequence) || typeof value.complete !== "boolean" || !Array.isArray(value.chunks) || value.chunks.length > 64) throw new Error("Invalid console page");
  let totalBytes = 0;
  const chunks = value.chunks.map((chunk: unknown): ConsoleChunk => {
    if (!object(chunk) || !integer(chunk.sequence) || chunk.sequence >= Number.MAX_SAFE_INTEGER || (chunk.stream !== "stdout" && chunk.stream !== "stderr") || !Array.isArray(chunk.bytes) || chunk.bytes.length < 1 || chunk.bytes.length > 65536 || !chunk.bytes.every((byte: unknown) => integer(byte) && byte <= 255)) throw new Error("Invalid console chunk");
    totalBytes += chunk.bytes.length;
    if (totalBytes > 262144) throw new Error("Console page exceeds byte limit");
    return { sequence: chunk.sequence, stream: chunk.stream, bytes: Uint8Array.from(chunk.bytes) };
  });
  if (value.dispatch_id === null && (value.dispatch_attempt !== 0 || chunks.length !== 0 || value.next_sequence !== 0 || value.complete)) throw new Error("Invalid queued console page");
  if (value.dispatch_id !== null && value.dispatch_attempt < 1) throw new Error("Invalid dispatch attempt");
  return { dispatchId: value.dispatch_id, dispatchAttempt: value.dispatch_attempt, chunks, nextSequence: value.next_sequence, complete: value.complete };
}

/** One sequential inclusive cursor, independent of run status and WS events. */
export class DurableConsoleModel {
  cursor = 0;
  dispatchId: string | null = null;
  dispatchAttempt: number | null = null;
  complete = false;
  private readonly decoder = new TextDecoder();
  private last: ConsolePage | null = null;
  private osc: "text" | "escape" | "osc" | "oscEscape" = "text";

  constructor(private readonly write: (text: string) => void) {}

  accept(input: unknown): ConsolePage {
    const page = parseConsolePage(input);
    if (this.dispatchId !== null && (page.dispatchId !== this.dispatchId || page.dispatchAttempt !== this.dispatchAttempt)) throw new ConsoleConflict("Console dispatch changed");
    const first = page.chunks[0];
    if (this.last && first && first.sequence < this.cursor) {
      const previous = this.last;
      const equal = page.dispatchId === previous.dispatchId && page.dispatchAttempt === previous.dispatchAttempt && page.nextSequence === previous.nextSequence && page.complete === previous.complete && page.chunks.length === previous.chunks.length && page.chunks.every((chunk, i) => { const other = previous.chunks[i]; return other !== undefined && chunk.sequence === other.sequence && chunk.stream === other.stream && chunk.bytes.length === other.bytes.length && chunk.bytes.every((b, j) => b === other.bytes[j]); });
      if (equal) return page;
      throw new Error("Conflicting console replay");
    }
    if (this.complete) throw new Error("Console already sealed");
    for (const [i, chunk] of page.chunks.entries()) if (chunk.sequence !== this.cursor + i) throw new Error("Console sequence gap");
    if (page.nextSequence !== this.cursor + page.chunks.length) throw new Error("Invalid console cursor");
    if (page.dispatchId !== null && this.dispatchId === null) { this.dispatchId = page.dispatchId; this.dispatchAttempt = page.dispatchAttempt; }
    this.cursor = page.nextSequence;
    this.complete = page.complete;
    this.last = page;
    for (const chunk of page.chunks) this.emit(this.decoder.decode(chunk.bytes, { stream: true }));
    if (page.complete) {
      this.emit(this.decoder.decode());
      if (this.osc === "escape") this.write("\x1b");
    }
    return page;
  }

  private emit(text: string) {
    let result = "";
    for (const char of text) {
      if (this.osc === "text") {
        if (char === "\x1b") this.osc = "escape";
        else if (char === "\u009d") this.osc = "osc";
        else result += char;
      } else if (this.osc === "escape") {
        if (char === "]") this.osc = "osc";
        else { result += "\x1b"; if (char === "\x1b") this.osc = "escape"; else if (char === "\u009d") this.osc = "osc"; else { result += char; this.osc = "text"; } }
      } else if (this.osc === "osc") {
        if (char === "\x07" || char === "\u009c") this.osc = "text";
        else if (char === "\x1b") this.osc = "oscEscape";
      } else if (char === "\\" || char === "\x07" || char === "\u009c") this.osc = "text";
      else this.osc = char === "\x1b" ? "oscEscape" : "osc";
    }
    if (result) this.write(result);
  }
}
