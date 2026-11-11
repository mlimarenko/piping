import { apiClient, unwrap } from "@/shared/api/client";
import { z } from "zod";

const revealedAuthKey = z.object({ value: z.string() });
export const fhrpAuthKeyIntent = z.object({
  intent: z.enum(["set", "rotate", "clear"]),
  value: z.string().min(1).optional(),
}).strict().refine(input => input.intent === "clear"
  ? input.value === undefined
  : input.value !== undefined);

export async function revealFhrpAuthKey(id: string): Promise<string> {
  const response = unwrap(await apiClient.POST("/api/v1/inventory/fhrp-groups/{id}/auth-key/reveal", {
    params: { path: { id } },
  }));
  return revealedAuthKey.parse(response).value;
}
