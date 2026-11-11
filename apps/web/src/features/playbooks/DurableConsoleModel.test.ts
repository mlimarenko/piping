import { describe, expect, it } from "vitest";
import { ConsoleConflict, DurableConsoleModel } from "./DurableConsoleModel";

const id = "00000000-0000-4000-8000-000000000198";
const page = (chunks: unknown[], next: number, complete = false, dispatch: string | null = id, attempt = 1) => ({ dispatch_id: dispatch, dispatch_attempt: attempt, chunks, next_sequence: next, complete });
const chunk = (sequence: number, bytes: number[], stream = "stdout") => ({ sequence, bytes, stream });

describe("durable console pages", () => {
  it("handles queued acquisition, interleaved chunks, exact replay and final UTF-8 flush", () => {
    let output = "";
    const model = new DurableConsoleModel((text) => { output += text; });
    model.accept(page([], 0, false, null, 0));
    const first = page([chunk(0, [27, 91, 51]), chunk(1, [49, 109, 0xe2], "stderr")], 2);
    model.accept(first); model.accept(first);
    model.accept(page([chunk(2, [0x82, 0xac, 13, 65], "stdout"), chunk(3, [0xe2], "stderr")], 4));
    expect(output).toBe("\x1b[31m€\rA");
    model.accept(page([], 4, true));
    expect(output).toBe("\x1b[31m€\rA�");
    expect(model.cursor).toBe(4);
    expect(model.complete).toBe(true);
  });

  it("validates the whole page and fails closed on identity and cursor changes", () => {
    let output = "";
    const model = new DurableConsoleModel((text) => { output += text; });
    expect(() => model.accept(page([chunk(0, [65]), chunk(2, [66])], 3))).toThrow();
    expect(output).toBe(""); expect(model.cursor).toBe(0);
    expect(() => model.accept(page([chunk(0, [256])], 1))).toThrow();
    expect(() => model.accept(page([chunk(0, [65])], 0))).toThrow();
    model.accept(page([chunk(0, [65])], 1));
    expect(() => model.accept(page([], 1, false, id, 2))).toThrow(ConsoleConflict);
    expect(() => model.accept(page([chunk(0, [66])], 1))).toThrow();
    expect(model.cursor).toBe(1); expect(output).toBe("A");
  });

  it("strips OSC even when escape sequences cross pages", () => {
    let output = "";
    const model = new DurableConsoleModel((text) => { output += text; });
    const encode = (s: string) => Array.from(new TextEncoder().encode(s));
    model.accept(page([chunk(0, encode("a\x1b]52;"))], 1));
    model.accept(page([chunk(1, encode("secret\x07b"))], 2, true));
    expect(output).toBe("ab");
  });

  it("rejects oversized pages and malformed queued attempts atomically", () => {
    let output = "";
    const model = new DurableConsoleModel((text) => { output += text; });
    const bytes = Array(65536).fill(65) as number[];
    expect(() => model.accept(page(Array.from({ length: 5 }, (_, i) => chunk(i, bytes)), 5))).toThrow();
    expect(() => model.accept(page([], 0, false, null, 1))).toThrow();
    expect(model.cursor).toBe(0);
    expect(model.dispatchId).toBeNull();
    expect(output).toBe("");
    model.accept(page(Array.from({ length: 4 }, (_, i) => chunk(i, bytes)), 4));
    expect(model.cursor).toBe(4);
  });
});
