import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

export type AutomationSecret = components["schemas"]["AutomationSecretResponse"];
export type ListAutomationSecretsResponse =
  components["schemas"]["ListAutomationSecretsResponse"];
export type CreateAutomationSecretRequest =
  components["schemas"]["CreateAutomationSecretRequest"];
export type UpdateAutomationSecretRequest =
  components["schemas"]["UpdateAutomationSecretRequest"];
/** Shared TanStack Query key for the named-secrets list. Every consumer (the
 *  admin page, the playbook editor picker, the policy resource multiselect)
 *  keys under this so a single invalidation refreshes them all. */
export const AUTOMATION_SECRETS_QUERY_KEY = ["automation-secrets"] as const;

export async function listAutomationSecrets(): Promise<AutomationSecret[]> {
  const response = unwrap(await apiClient.GET("/api/v1/automation-secrets", {}));
  return response.items;
}

export async function createAutomationSecret(
  body: CreateAutomationSecretRequest,
): Promise<AutomationSecret> {
  return unwrap(await apiClient.POST("/api/v1/automation-secrets", { body }));
}

export async function updateAutomationSecret(
  id: string,
  body: UpdateAutomationSecretRequest,
): Promise<AutomationSecret> {
  return unwrap(
    await apiClient.PATCH("/api/v1/automation-secrets/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteAutomationSecret(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/automation-secrets/{id}", {
    params: { path: { id } },
  });
}

/** Write-only: sets (or rotates) the secret's value. The value is never read
 *  back; the caller only learns `has_value` from the list. Returns 204, so
 *  there is no body to unwrap. */
export async function setAutomationSecretValue(
  id: string,
  value: string,
): Promise<void> {
  await apiClient.PUT("/api/v1/secrets/{id}/value", {
    params: { path: { id } },
    body: { value },
  });
}

/** Clears the stored value (`has_value` returns to false). */
export async function clearAutomationSecretValue(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/secrets/{id}/value", {
    params: { path: { id } },
  });
}
