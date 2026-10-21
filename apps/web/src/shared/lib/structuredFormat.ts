import { stringify as stringifyYaml } from "yaml";
import { stringify as stringifyToml } from "smol-toml";

import type { ConfigContextFormat } from "@/shared/settings/registry";

export interface SerializedStructured {
  /** The serialized text in the requested (or fallback) format. */
  text: string;
  /**
   * Set when the requested format could not represent the value and the result
   * fell back to JSON (e.g. TOML cannot encode `null` or mixed-type arrays).
   */
  fellBackToJson?: boolean;
}

function toJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? "{}";
  } catch {
    return "{}";
  }
}

/**
 * True when the value contains a `null`/`undefined` anywhere. TOML has no null,
 * and `smol-toml` silently DROPS such keys instead of erroring — which would
 * misrepresent the rendered context — so we detect them and fall back to JSON.
 */
function hasNullish(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (Array.isArray(value)) return value.some(hasNullish);
  if (typeof value === "object") {
    return Object.values(value as Record<string, unknown>).some(hasNullish);
  }
  return false;
}

/**
 * Serialize a JSON-decoded value for read-only display in one of the supported
 * formats. JSON and YAML accept any decoded value; TOML requires a top-level
 * table and cannot encode `null`/`undefined` or mixed-type arrays, so on any
 * TOML/YAML serialization error we fall back to JSON and flag it (we never strip
 * data to force a format — that would misrepresent the rendered context).
 */
export function serializeStructured(
  value: unknown,
  format: ConfigContextFormat,
): SerializedStructured {
  const safe = value ?? {};

  if (format === "json") {
    return { text: toJson(safe) };
  }

  if (format === "yaml") {
    try {
      return { text: stringifyYaml(safe) };
    } catch {
      return { text: toJson(safe), fellBackToJson: true };
    }
  }

  // TOML: only a top-level object is representable as a document, and the value
  // must not contain any null (which TOML cannot encode).
  if (typeof safe !== "object" || safe === null || Array.isArray(safe) || hasNullish(safe)) {
    return { text: toJson(safe), fellBackToJson: true };
  }
  try {
    return { text: stringifyToml(safe) };
  } catch {
    return { text: toJson(safe), fellBackToJson: true };
  }
}
