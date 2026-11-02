import { apiClient, unwrap, type components } from "@/shared/api/client";

export type AssistantBootstrap = components["schemas"]["AssistantBootstrapResponse"];
export type AssistantSession = components["schemas"]["AssistantSession"];
export type AssistantSessionState = components["schemas"]["AssistantSessionState"];
export type AssistantMessage = components["schemas"]["AssistantMessage"];
export type CreateAssistantSessionResponse =
  components["schemas"]["CreateAssistantSessionResponse"];

export async function fetchAssistantBootstrap(): Promise<AssistantBootstrap> {
  return unwrap(await apiClient.GET("/api/v1/assistant/bootstrap", {}));
}

export async function fetchAssistantSessions(): Promise<AssistantSession[]> {
  const response = unwrap(await apiClient.GET("/api/v1/assistant/sessions", {}));
  return response.sessions;
}

export async function createAssistantSession(
  title?: string,
): Promise<CreateAssistantSessionResponse> {
  return unwrap(
    await apiClient.POST("/api/v1/assistant/sessions", {
      body: { title },
    }),
  );
}

export async function fetchAssistantSessionState(
  id: string,
): Promise<AssistantSessionState> {
  return unwrap(
    await apiClient.GET("/api/v1/assistant/sessions/{id}/state", {
      params: { path: { id } },
    }),
  );
}

export async function updateAssistantSession(
  id: string,
  body: { title?: string; pinned?: boolean },
): Promise<AssistantSession> {
  return unwrap(
    await apiClient.PATCH("/api/v1/assistant/sessions/{id}", {
      params: { path: { id } },
      body,
    }),
  );
}

export async function archiveAssistantSession(id: string): Promise<void> {
  await apiClient.DELETE("/api/v1/assistant/sessions/{id}", {
    params: { path: { id } },
  });
}

export async function sendAssistantMessage(
  id: string,
  text: string,
): Promise<void> {
  await apiClient.POST("/api/v1/assistant/sessions/{id}/messages", {
    params: { path: { id } },
    body: { text },
  });
}
