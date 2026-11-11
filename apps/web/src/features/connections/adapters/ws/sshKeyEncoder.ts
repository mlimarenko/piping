export type SshKeyModifiers = { ctrl?: boolean; alt?: boolean; shift?: boolean };

export type SshTerminalKey =
  | "enter" | "tab" | "escape" | "interrupt" | "pageUp" | "pageDown"
  | "up" | "down" | "left" | "right" | "home" | "end" | "insert" | "delete" | "backspace"
  | "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12"
  | { character: string };

const cursorKeys = { up: "A", down: "B", right: "C", left: "D", home: "H", end: "F" } as const;
const functionKeys = { f1: "P", f2: "Q", f3: "R", f4: "S" } as const;
const tildeKeys = { f5: 15, f6: 17, f7: 18, f8: 19, f9: 20, f10: 21, f11: 23, f12: 24, delete: 3 } as const;
const shifted = ")!@#$%^&*(";
const punctuation = "`;=,-./[\\]'";
const shiftedPunctuation = "~:+<_>?{|}\"";

/** Mirrors xterm 6's Keyboard.ts; unsupported chords are not sent as text. */
export function encodeSshKey(key: SshTerminalKey, modifiers: SshKeyModifiers = {}, applicationCursor = false): string | undefined {
  const { ctrl = false, alt = false, shift = false } = modifiers;
  const bits = (shift ? 1 : 0) | (alt ? 2 : 0) | (ctrl ? 4 : 0);
  if (typeof key === "object" && key !== null) {
    const char = key.character;
    if (typeof char !== "string" || !/^[^\p{C}\p{M}\p{Zl}\p{Zp}]$/u.test(char)) return undefined;
    if (!bits) return char;
    if (!/^[\x20-\x7e]$/.test(char)) return undefined;
    // xterm's Alt/Meta branch keeps Ctrl letters/space even with Shift;
    // digits and punctuation use the ordinary Alt mapping below, not Ctrl codes.
    if (ctrl && (!alt || /^[a-zA-Z ]$/.test(char))) {
      if (shift && !alt && char !== "@" && char !== "_") return undefined;
      let code: number | undefined;
      if (/^[a-zA-Z]$/.test(char)) code = char.toUpperCase().charCodeAt(0) - 64;
      else if (char === " " || char === "@") code = 0;
      else if (char >= "3" && char <= "7") code = char.charCodeAt(0) - 51 + 27;
      else if (char === "8") code = 127;
      else if (char === "[") code = 27;
      else if (char === "\\") code = 28;
      else if (char === "]") code = 29;
      else if (char === "_") code = 31;
      return code === undefined ? undefined : (alt ? "\x1b" : "") + String.fromCharCode(code);
    }
    let result = char;
    if (shift) {
      if (/^[a-zA-Z]$/.test(char)) result = char.toUpperCase();
      else if (/^[0-9]$/.test(char)) result = shifted[Number(char)]!;
      else if (punctuation.includes(char)) result = shiftedPunctuation[punctuation.indexOf(char)]!;
    }
    return (alt ? "\x1b" : "") + result;
  }
  if (typeof key !== "string") return undefined;
  if (Object.hasOwn(cursorKeys, key)) {
    const suffix = cursorKeys[key as keyof typeof cursorKeys];
    return bits ? `\x1b[1;${bits + 1}${suffix}` : `\x1b${applicationCursor ? "O" : "["}${suffix}`;
  }
  if (Object.hasOwn(functionKeys, key)) {
    const suffix = functionKeys[key as keyof typeof functionKeys];
    return bits ? `\x1b[1;${bits + 1}${suffix}` : `\x1bO${suffix}`;
  }
  if (Object.hasOwn(tildeKeys, key)) {
    const number = tildeKeys[key as keyof typeof tildeKeys];
    return `\x1b[${number}${bits ? `;${bits + 1}` : ""}~`;
  }
  switch (key) {
    case "enter": return alt ? "\x1b\r" : "\r";
    case "escape": return alt ? "\x1b\x1b" : "\x1b";
    case "interrupt": return bits ? undefined : "\x03";
    case "tab": return shift ? "\x1b[Z" : "\t";
    case "backspace": return (alt ? "\x1b" : "") + (ctrl ? "\b" : "\x7f");
    case "insert": return shift || ctrl ? undefined : "\x1b[2~";
    case "pageUp":
    case "pageDown": {
      if (shift) return undefined; // xterm scrolls locally instead of sending a key.
      const number = key === "pageUp" ? 5 : 6;
      return `\x1b[${number}${ctrl ? `;${bits + 1}` : ""}~`;
    }
    default: return undefined;
  }
}
