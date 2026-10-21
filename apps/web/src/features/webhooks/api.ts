import { apiClient, unwrap } from "@/shared/api/client";
import type { components, operations } from "@/shared/api/schema.gen";

/**
 * One subscribable event from the backend event catalog. `value` is the real
 * `event_type` string (e.g. `inventory.tenant.created`); `group`/`object`/
 * `action` drive UI grouping and presets.
 */
export type WebhookEventCatalogItem = components["schemas"]["WebhookEventCatalogItemResponse"];

export type WebhookSubscription = components["schemas"]["WebhookSubscriptionResponse"];

export type WebhookSubscriptionListResponse = components["schemas"]["WebhookSubscriptionListResponse"];

export type CreateWebhookSubscriptionRequest = components["schemas"]["CreateWebhookSubscriptionRequest"];

export type UpdateWebhookSubscriptionRequest = components["schemas"]["UpdateWebhookSubscriptionRequest"];

export type TestFireResponse = components["schemas"]["TestFireResponse"];

/**
 * List webhook subscriptions.
 *
 * Forwards the backend allowlisted sort for every visible data column,
 * comma-separated multi-value table filters, and server-side free-text search.
 */
export async function listWebhooks(
  params?: {
    limit?: number;
    offset?: number;
    search?: string;
    sort?: string;
    order?: "asc" | "desc";
    /** Comma-separated `true`/`false`; matches any (`IN`). */
    enabled?: string;
    /** Comma-separated `true`/`false`; matches any (`IN`) against secret presence. */
    secret_set?: string;
    /** Comma-separated event type values; matches any subscription overlap. */
    event_types?: string;
    /** Comma-separated HTTP status codes; matches any non-null last status. */
    last_status?: string;
  },
): Promise<WebhookSubscriptionListResponse> {
  const query: operations["list_webhook_subscriptions"]["parameters"]["query"] = {
    limit: params?.limit,
    offset: params?.offset,
    search: params?.search,
    enabled: params?.enabled,
    secret_set: params?.secret_set,
    event_types: params?.event_types,
    last_status: params?.last_status,
    ...(params?.sort ? { sort: params.sort, order: params.order ?? "asc" } : {}),
  };
  return unwrap(
    await apiClient.GET("/api/v1/webhooks", {
      params: { query },
    }),
  );
}

export async function createWebhook(
  body: CreateWebhookSubscriptionRequest,
): Promise<WebhookSubscription> {
  return unwrap(await apiClient.POST("/api/v1/webhooks", { body }));
}

export async function updateWebhook(
  id: string,
  body: UpdateWebhookSubscriptionRequest,
): Promise<WebhookSubscription> {
  return unwrap(
    await apiClient.PUT("/api/v1/webhooks/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function deleteWebhook(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/webhooks/{id}", {
    params: { path: { id } },
  });
}

export async function testFireWebhook(id: string): Promise<TestFireResponse> {
  return unwrap(
    await apiClient.POST("/api/v1/webhooks/{id}/test-fire", {
      params: { path: { id } },
    }),
  );
}

export async function clearWebhookSecret(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/webhooks/{id}/secret", {
    params: { path: { id } },
  });
}

export async function getWebhookEventCatalog(): Promise<WebhookEventCatalogItem[]> {
  const res = unwrap(await apiClient.GET("/api/v1/webhooks/event-catalog"));
  return res.items;
}
