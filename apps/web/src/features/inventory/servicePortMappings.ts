import { z } from "zod";

export const servicePortMappingsSchema = z.array(
  z.string().regex(/^(tcp|udp|sctp)\/[0-9]+$/i)
    .refine((mapping) => {
      const port = Number(mapping.split("/")[1]);
      return port >= 1 && port <= 65535;
    })
    .transform((mapping) => `${mapping.slice(0, mapping.indexOf("/")).toLowerCase()}/${Number(mapping.split("/")[1])}`),
).min(1).refine((mappings) => new Set(mappings).size === mappings.length);

export function parseServicePortMappings(value: string): string[] | null {
  const result = servicePortMappingsSchema.safeParse(value.split(",").map((mapping) => mapping.trim()));
  return result.success ? result.data : null;
}
