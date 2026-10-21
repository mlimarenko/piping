/** Explicit flow intent only: never infer ports from endpoint inventory. */
export function parseDataflowPorts(input: string): number[] | null {
  const tokens = input.split(",").map(token => token.trim());
  if (!tokens.length || tokens.some(token => !/^\d+$/.test(token))) return null;
  const ports = tokens.map(Number);
  if (ports.some(port => !Number.isInteger(port) || port < 1 || port > 65535)) return null;
  const distinct = [...new Set(ports)].sort((a, b) => a - b);
  return distinct.length > 256 ? null : distinct;
}
