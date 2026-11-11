import type { Resource } from "@/shared/api/types";

/** Native Interface attributes; RF channel tokens follow NetBox 4.7.1. */
export const POE_MODES = ["pd", "pse"] as const;
export const POE_TYPES = ["type1-ieee802.3af", "type2-ieee802.3at", "type3-ieee802.3bt", "type4-ieee802.3bt", "passive-24v-2pair", "passive-24v-4pair", "passive-48v-2pair", "passive-48v-4pair"] as const;
export const RF_ROLES = ["ap", "station"] as const;

export const interfaceNetworkingFields = [
  { key: "mtu", wire: "mtu", min: 1, max: 65536 },
  { key: "mode", wire: "mode", options: ["access", "tagged", "tagged-all", "q-in-q"] },
  // Native REST uses a decimal string for the complete positive bigint range.
  { key: "speed", wire: "speed" },
  { key: "duplex", wire: "duplex", options: ["half", "full", "auto"] },
  { key: "poeMode", wire: "poe_mode", options: POE_MODES },
  { key: "poeType", wire: "poe_type", options: POE_TYPES },
  { key: "rfRole", wire: "rf_role", options: RF_ROLES },
  { key: "rfChannel", wire: "rf_channel" },
  { key: "rfChannelFrequency", wire: "rf_channel_frequency", min: -99999.999, max: 99999.999, decimal: true },
  { key: "rfChannelWidth", wire: "rf_channel_width", min: -9999.999, max: 9999.999, decimal: true },
  { key: "txPower", wire: "tx_power", min: -40, max: 127 },
  { key: "wwn", wire: "wwn" },
  { key: "channelId", wire: "channel_id", min: 1, max: 1024 },
  { key: "channels", wire: "channels", min: 1, max: 1024 },
  { key: "cableConnector", wire: "cable_connector", min: 1, max: 256 },
  { key: "cableEnd", wire: "cable_end", options: ["A", "B"] },
  { key: "cablePositions", wire: "cable_positions" },
] as const;

export type InterfaceNetworkingKey = typeof interfaceNetworkingFields[number]["key"];

/** Reject lossy or malformed form input rather than dropping it from a write. */
export function parseInterfaceNetworkingValue(key: InterfaceNetworkingKey, value: string | null | undefined): string | number | number[] | null | undefined {
  if (value === undefined) return undefined;
  if (value === null || value.trim() === "") return null;
  const text = value.trim();
  if (key === "speed") {
    if (!/^(0|[1-9]\d{0,18})$/.test(text) || BigInt(text) > 9223372036854775807n) throw new Error(key);
    return text;
  }
  const field = interfaceNetworkingFields.find((entry) => entry.key === key)!;
  if ("options" in field && !(field.options as readonly string[]).includes(text)) throw new Error(key);
  if ("min" in field) {
    const number = Number(text);
    const decimal = "decimal" in field;
    if (!Number.isFinite(number) || number < field.min || number > field.max ||
      (decimal ? !/^-?\d+(?:\.\d{1,3})?$/.test(text) : !Number.isSafeInteger(number))) throw new Error(key);
    return number;
  }
  if (key === "wwn") {
    if (!/^(?:[0-9a-f]{2}:){7}[0-9a-f]{2}$/i.test(text)) throw new Error(key);
    return text.toLowerCase();
  }
  if (key === "cablePositions") {
    const parts = text.split(",").map((part) => part.trim());
    if (parts.some((part) => !/^\d+$/.test(part) || Number(part) < 1 || Number(part) > 1024)) throw new Error(key);
    return parts.map(Number);
  }
  return text;
}

export function interfaceNetworkingPayload(body: Partial<Resource>): Record<string, unknown> {
  return Object.fromEntries(interfaceNetworkingFields.flatMap(({ key, wire }) => {
    const value = parseInterfaceNetworkingValue(key, body[key]);
    return value === undefined ? [] : [[wire, value]];
  }));
}
