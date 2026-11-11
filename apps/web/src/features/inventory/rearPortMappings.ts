/**
 * FrontPortTemplate rear-port mappings.
 *
 * The native write contract is an ordered array of
 * `{ position, rear_port, rear_port_position }`: omission keeps the stored
 * mappings, `[]` clears them and `null` is rejected. The form keeps one canonical
 * serialization in its draft; the structured editor is the only producer.
 */

export interface RearPortMapping {
  position: number;
  rearPortId: string;
  rearPortPosition: number;
}

export interface FrontPortMappingProjection {
  /** Front-side position (wire `front_port_position`). */
  frontPosition: number;
  frontPortId: string;
  /** Rear-side position (wire `position`). */
  rearPosition: number;
}

const MAX_POSITION = 1024;

const isInteger = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= MAX_POSITION;

export function parseRearPortMappings(
  value: string | null | undefined,
): RearPortMapping[] | undefined {
  if (value === undefined) return undefined;
  if (value === null) throw new Error("Rear port mappings cannot be null");
  if (value.trim() === "") return [];
  let raw: unknown;
  try {
    raw = JSON.parse(value);
  } catch {
    throw new Error("Invalid rear port mappings");
  }
  if (!Array.isArray(raw)) throw new Error("Invalid rear port mappings");
  return raw.map((entry) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      throw new Error("Invalid rear port mapping row");
    }
    const row = entry as Record<string, unknown>;
    if (Object.keys(row).length !== 3) throw new Error("Invalid rear port mapping row");
    if (typeof row.rearPortId !== "string" || row.rearPortId.trim() === "") {
      throw new Error("Invalid rear port mapping target");
    }
    if (!isInteger(row.position) || !isInteger(row.rearPortPosition)) {
      throw new Error("Invalid rear port mapping position");
    }
    return { position: row.position, rearPortId: row.rearPortId, rearPortPosition: row.rearPortPosition };
  });
}

export function serializeRearPortMappings(rows: readonly RearPortMapping[]): string {
  return rows.length === 0 ? "" : JSON.stringify(rows);
}

/** Wire rows for the native `rear_ports` member (never `null`). */
export function rearPortMappingsWire(
  rows: readonly RearPortMapping[],
): Array<{ position: number; rear_port: string; rear_port_position: number }> {
  return rows.map((row) => ({
    position: row.position,
    rear_port: row.rearPortId,
    rear_port_position: row.rearPortPosition,
  }));
}

/** Read side: native `rear_ports` rows become the canonical draft serialization. */
export function rearPortMappingsFromWire(
  rows: Array<{ position: number; rear_port: string; rear_port_position: number }> | null | undefined,
): string {
  return serializeRearPortMappings((rows ?? []).map((row) => ({
    position: row.position,
    rearPortId: row.rear_port,
    rearPortPosition: row.rear_port_position,
  })));
}

/** Read-only reverse projection shown on RearPortTemplate rows. */
export function parseFrontPortMappings(value: string | undefined): FrontPortMappingProjection[] {
  if (!value) return [];
  try {
    const raw: unknown = JSON.parse(value);
    if (!Array.isArray(raw)) return [];
    return raw.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const row = entry as Record<string, unknown>;
      if (typeof row.frontPortId !== "string" || !isInteger(row.position) || !isInteger(row.frontPortPosition)) return [];
      return [{ frontPosition: row.frontPortPosition, frontPortId: row.frontPortId, rearPosition: row.position }];
    });
  } catch {
    return [];
  }
}

export function frontPortMappingsFromWire(
  rows: Array<{ position: number; front_port: string; front_port_position: number }> | null | undefined,
): string {
  return JSON.stringify((rows ?? []).map((row) => ({
    position: row.position,
    frontPortId: row.front_port,
    frontPortPosition: row.front_port_position,
  })));
}

/** Row-level bounds: front position within this port's positions, rear position
 *  within the selected rear port's declared positions. */
export function rearPortMappingError(
  row: RearPortMapping,
  frontPositions: number,
  rearPositions: number,
): "position" | "rearPosition" | null {
  if (!Number.isInteger(row.position) || row.position < 1 || row.position > frontPositions) return "position";
  if (!Number.isInteger(row.rearPortPosition) || row.rearPortPosition < 1 || row.rearPortPosition > rearPositions) return "rearPosition";
  return null;
}
