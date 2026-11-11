import { expect, it, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import { findConsoleText } from "./NativeRunConsoleModel";

it("selects a Unicode match in the actual terminal buffer", async () => {
  const terminal = new Terminal({ cols: 30, rows: 3 });
  await new Promise<void>((resolve) => terminal.write("hello 世界\r\nsecond", resolve));
  const select = vi.spyOn(terminal, "select").mockImplementation(() => {});
  vi.spyOn(terminal, "scrollToLine").mockImplementation(() => {});
  expect(findConsoleText(terminal, "世界", 0)).toBe(0);
  expect(select).toHaveBeenCalledWith(6, 0, 4);
  expect(findConsoleText(terminal, "missing", 0)).toBeNull();
  terminal.dispose();
});
