const MAX_VALUES = 256;
const PORT_RANGE = /^(\d+)-(\d+)$/;

/** Flow-owned port intent only: never infer ports from endpoint inventory.
 * Accepts the historical comma-separated integers and inclusive `start-end`
 * ranges. Invalid or out-of-bound input returns null instead of widening. */
export function parseDataflowPorts(input: string): number[] | null {
  const tokens = input.split(",").map(token => token.trim());
  if (!tokens.length || tokens.some(token => !token)) return null;
  const ports = new Set<number>();
  for (const token of tokens) {
    if (/^\d+$/.test(token)) {
      const port = Number(token);
      if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
      ports.add(port);
    } else {
      const match = PORT_RANGE.exec(token);
      if (!match) return null;
      const start = Number(match[1]);
      const end = Number(match[2]);
      if (!Number.isInteger(start) || !Number.isInteger(end) || start < 1 || end > 65535 || start > end) return null;
      // A range wider than the bounded representation is rejected, never clamped.
      if (end - start + 1 > MAX_VALUES) return null;
      for (let port = start; port <= end; port += 1) ports.add(port);
    }
    if (ports.size > MAX_VALUES) return null;
  }
  return [...ports].sort((left, right) => left - right);
}

/** ICMP types are bounded integers 0..255; null is invalid input, not "all". */
export function parseDataflowIcmpTypes(input: string): number[] | null {
  const tokens = input.split(",").map(token => token.trim());
  if (!tokens.length || tokens.some(token => !token)) return null;
  const types = new Set<number>();
  for (const token of tokens) {
    if (!/^\d+$/.test(token)) return null;
    const type = Number(token);
    if (!Number.isInteger(type) || type < 0 || type > 255) return null;
    types.add(type);
  }
  return [...types].sort((left, right) => left - right);
}
