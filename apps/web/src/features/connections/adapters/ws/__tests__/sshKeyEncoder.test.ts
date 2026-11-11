import { describe, expect, it } from "vitest";
import { encodeSshKey, type SshTerminalKey } from "../sshKeyEncoder";

describe("SSH virtual key encoding (xterm 6 keyboard sequences)", () => {
  const combinations = Array.from({ length: 8 }, (_, bits) => ({
    bits,
    modifiers: { shift: !!(bits & 1), alt: !!(bits & 2), ctrl: !!(bits & 4) },
  }));

  it.each(combinations)("matches cursor, function and navigation keys for modifier bits $bits", ({ bits, modifiers }) => {
    const cursors = { up: "A", down: "B", right: "C", left: "D", home: "H", end: "F" } as const;
    for (const [key, suffix] of Object.entries(cursors)) {
      for (const application of [false, true]) {
        const expected = bits ? `\x1b[1;${bits + 1}${suffix}` : `\x1b${application ? "O" : "["}${suffix}`;
        expect(encodeSshKey(key as SshTerminalKey, modifiers, application)).toBe(expected);
      }
    }
    const functions = ["P", "Q", "R", "S", "15", "17", "18", "19", "20", "21", "23", "24"];
    functions.forEach((suffix, index) => {
      const expected = index < 4
        ? (bits ? `\x1b[1;${bits + 1}${suffix}` : `\x1bO${suffix}`)
        : `\x1b[${suffix}${bits ? `;${bits + 1}` : ""}~`;
      expect(encodeSshKey(`f${index + 1}` as SshTerminalKey, modifiers)).toBe(expected);
    });
    expect(encodeSshKey("delete", modifiers)).toBe(`\x1b[3${bits ? `;${bits + 1}` : ""}~`);
    expect(encodeSshKey("insert", modifiers)).toBe(modifiers.shift || modifiers.ctrl ? undefined : "\x1b[2~");
    for (const [key, number] of [["pageUp", 5], ["pageDown", 6]] as const) {
      // Shift-Page scrolls xterm locally; it is not a PTY key.
      const expected = modifiers.shift ? undefined : `\x1b[${number}${modifiers.ctrl ? `;${bits + 1}` : ""}~`;
      expect(encodeSshKey(key, modifiers)).toBe(expected);
    }
    expect(encodeSshKey("tab", modifiers)).toBe(modifiers.shift ? "\x1b[Z" : "\t");
    expect(encodeSshKey("enter", modifiers)).toBe(modifiers.alt ? "\x1b\r" : "\r");
    expect(encodeSshKey("escape", modifiers)).toBe(modifiers.alt ? "\x1b\x1b" : "\x1b");
    expect(encodeSshKey("backspace", modifiers)).toBe(`${modifiers.alt ? "\x1b" : ""}${modifiers.ctrl ? "\b" : "\x7f"}`);
    expect(encodeSshKey("interrupt", modifiers)).toBe(bits ? undefined : "\x03");
  });

  it.each([
    ["up", "A"], ["down", "B"], ["right", "C"], ["left", "D"], ["home", "H"], ["end", "F"],
  ] as const)("encodes %s in both cursor modes and with modifiers", (key, suffix) => {
    expect(encodeSshKey(key)).toBe(`\x1b[${suffix}`);
    expect(encodeSshKey(key, {}, true)).toBe(`\x1bO${suffix}`);
    expect(encodeSshKey(key, { shift: true, alt: true, ctrl: true }, true)).toBe(`\x1b[1;8${suffix}`);
  });

  it.each([
    ["f1", "OP", "1;5P"], ["f2", "OQ", "1;5Q"], ["f3", "OR", "1;5R"], ["f4", "OS", "1;5S"],
    ["f5", "[15~", "15;5~"], ["f6", "[17~", "17;5~"], ["f7", "[18~", "18;5~"], ["f8", "[19~", "19;5~"],
    ["f9", "[20~", "20;5~"], ["f10", "[21~", "21;5~"], ["f11", "[23~", "23;5~"], ["f12", "[24~", "24;5~"],
  ] as const)("encodes %s and Ctrl-%s", (key, normal, modified) => {
    expect(encodeSshKey(key)).toBe(`\x1b${normal}`);
    expect(encodeSshKey(key, { ctrl: true })).toBe(`\x1b[${modified}`);
  });

  it("uses xterm navigation and control-key rules", () => {
    expect(encodeSshKey("delete", { alt: true, shift: true })).toBe("\x1b[3;4~");
    expect(encodeSshKey("insert", { alt: true })).toBe("\x1b[2~");
    expect(encodeSshKey("insert", { ctrl: true })).toBeUndefined();
    expect(encodeSshKey("pageUp", { ctrl: true, alt: true })).toBe("\x1b[5;7~");
    expect(encodeSshKey("pageDown", { shift: true })).toBeUndefined();
    expect(encodeSshKey("tab", { shift: true, ctrl: true })).toBe("\x1b[Z");
    expect(encodeSshKey("backspace", { ctrl: true, alt: true })).toBe("\x1b\b");
    expect(encodeSshKey("enter", { alt: true })).toBe("\x1b\r");
    expect(encodeSshKey("escape", { alt: true })).toBe("\x1b\x1b");
  });

  it("accepts a single printable character and rejects composition or unsupported chords", () => {
    expect(encodeSshKey({ character: "я" })).toBe("я");
    expect(encodeSshKey({ character: "c" }, { ctrl: true })).toBe("\x03");
    expect(encodeSshKey({ character: "x" }, { ctrl: true, alt: true })).toBe("\x1b\x18");
    expect(encodeSshKey({ character: "1" }, { alt: true, shift: true })).toBe("\x1b!");
    for (const character of ["", "abc", "e\u0301", "\n", "\u0301", "👩‍💻"]) {
      expect(encodeSshKey({ character })).toBeUndefined();
    }
    expect(encodeSshKey({ character: "я" }, { ctrl: true })).toBeUndefined();
    expect(encodeSshKey({ character: "?" }, { ctrl: true })).toBeUndefined();
    expect(encodeSshKey("unknown" as SshTerminalKey)).toBeUndefined();
  });

  it.each([
    ["a", "\x01"], ["A", "\x01"], ["c", "\x03"], ["C", "\x03"],
    ["z", "\x1a"], ["Z", "\x1a"], [" ", "\x00"],
  ])("encodes Ctrl+Alt character %j with or without Shift", (character, control) => {
    for (const shift of [false, true]) {
      expect(encodeSshKey({ character }, { ctrl: true, alt: true, shift })).toBe(`\x1b${control}`);
    }
    // Without Alt, xterm does not send Ctrl+Shift letters or space.
    expect(encodeSshKey({ character }, { ctrl: true, shift: true })).toBeUndefined();
  });

  // xterm Keyboard.ts's Alt branch maps these keys before considering Ctrl.
  it.each([
    ["0", ")"], ["1", "!"], ["2", "@"], ["3", "#"], ["4", "$"],
    ["5", "%"], ["6", "^"], ["7", "&"], ["8", "*"], ["9", "("],
    [";", ":"], ["=", "+"], [",", "<"], ["-", "_"], [".", ">"],
    ["/", "?"], ["`", "~"], ["[", "{"], ["\\", "|"], ["]", "}"], ["'", "\""],
  ])("preserves the Alt mapping for Ctrl+Alt character %j", (character, shifted) => {
    expect(encodeSshKey({ character }, { ctrl: true, alt: true })).toBe(`\x1b${character}`);
    expect(encodeSshKey({ character }, { ctrl: true, alt: true, shift: true })).toBe(`\x1b${shifted}`);
  });

  it("rejects composed, multi-character and non-ASCII triple-modifier input", () => {
    for (const character of ["", "cc", "e\u0301", "\u0301", "я", "漢", "😀", "\n", "\x00"]) {
      expect(encodeSshKey({ character }, { ctrl: true, alt: true, shift: true })).toBeUndefined();
    }
    expect(encodeSshKey({ character: "?" }, { ctrl: true, shift: true })).toBeUndefined();
    expect(encodeSshKey({ character: "@" }, { ctrl: true, shift: true })).toBe("\x00");
    expect(encodeSshKey({ character: "_" }, { ctrl: true, shift: true })).toBe("\x1f");
  });
});
