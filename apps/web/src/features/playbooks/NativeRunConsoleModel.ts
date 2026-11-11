import type { Terminal } from "@xterm/xterm";

export interface NativeRunChunk {
  sequence: number;
  bytes: Uint8Array;
}

export function findConsoleText(terminal: Terminal, query: string, startRow: number): number | null {
  if (!query) return null;
  const buffer = terminal.buffer.active;
  for (let step = 0; step < buffer.length; step++) {
    const row = (startRow + step) % buffer.length;
    const line = buffer.getLine(row);
    if (!line) continue;
    let text = "";
    const columns: number[] = [];
    for (let col = 0; col < line.length; col++) {
      const cell = line.getCell(col);
      if (!cell || cell.getWidth() === 0) continue;
      const chars = cell.getChars() || " ";
      for (let i = 0; i < chars.length; i++) columns.push(col);
      text += chars;
    }
    const index = text.indexOf(query);
    if (index < 0) continue;
    const start = columns[index];
    const end = columns[index + query.length - 1];
    if (start === undefined || end === undefined) continue;
    terminal.select(start, row, end - start + (line.getCell(end)?.getWidth() ?? 1));
    terminal.scrollToLine(row);
    return row;
  }
  return null;
}
