import {
  useCallback,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import {
  archiveAssistantSession,
  createAssistantSession,
  fetchAssistantBootstrap,
  fetchAssistantSessionState,
  fetchAssistantSessions,
  sendAssistantMessage,
  updateAssistantSession,
} from "./api";
import { AssistantContext, type AssistantContextValue } from "./assistantContext";
import { notify } from "@/shared/lib/notifications";
import {
  DEFAULT_ASSISTANT_WINDOW_SETTING,
  normalizeAssistantWindowSetting,
  type AssistantWindowDock,
  type AssistantWindowSettingValue,
} from "@/shared/settings/registry";
import { useSetting } from "@/shared/settings/hooks";

export function AssistantProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const windowSetting = useSetting<AssistantWindowSettingValue>(
    "assistant.window",
    DEFAULT_ASSISTANT_WINDOW_SETTING,
    {
      debounceMs: 250,
      normalize: normalizeAssistantWindowSetting,
    },
  );
  const setAssistantWindow = windowSetting.setValue;

  const bootstrapQuery = useQuery({
    queryKey: ["assistant", "bootstrap"],
    queryFn: fetchAssistantBootstrap,
    retry: false,
    staleTime: 30_000,
  });

  const canUseSessionShell = Boolean(bootstrapQuery.data?.capabilities?.session_shell);
  const canUpdatePreferences = Boolean(bootstrapQuery.data?.access?.can_update_preferences);

  // Ready only when bootstrap has loaded successfully AND the assistant is fully configured AND
  // the user has Execute (chat) permission. Never true while loading or on error.
  const assistantReady = useMemo(
    () =>
      !bootstrapQuery.isLoading &&
      !bootstrapQuery.isError &&
      bootstrapQuery.data !== undefined &&
      Boolean(bootstrapQuery.data.capabilities?.session_shell) &&
      bootstrapQuery.data.config?.provider != null &&
      bootstrapQuery.data.config?.model != null &&
      Boolean(bootstrapQuery.data.config?.api_key_configured) &&
      Boolean(bootstrapQuery.data.access?.can_execute),
    [bootstrapQuery.isLoading, bootstrapQuery.isError, bootstrapQuery.data],
  );

  const sessionsQuery = useQuery({
    queryKey: ["assistant", "sessions"],
    queryFn: fetchAssistantSessions,
    enabled: canUseSessionShell,
    retry: false,
    staleTime: 15_000,
  });

  const sessions = useMemo(
    () => sessionsQuery.data ?? [],
    [sessionsQuery.data],
  );
  const activeSessions = useMemo(
    () => sessions.filter((session) => session.lifecycle === "active"),
    [sessions],
  );
  const selectedSession =
    sessions.find((session) => session.id === selectedSessionId) ?? undefined;

  const resolvedSessionId: string | null = canUseSessionShell
    ? (selectedSessionId && sessions.some((s) => s.id === selectedSessionId)
        ? selectedSessionId
        : (activeSessions[0]?.id ?? sessions[0]?.id ?? null))
    : null;
  if (resolvedSessionId !== selectedSessionId) {
    setSelectedSessionId(resolvedSessionId);
  }

  const sessionStateQuery = useQuery({
    queryKey: ["assistant", "sessions", selectedSessionId, "state"],
    queryFn: () => fetchAssistantSessionState(selectedSessionId as string),
    enabled: canUseSessionShell && Boolean(selectedSessionId),
    retry: false,
    staleTime: 5_000,
  });

  const createMutation = useMutation({
    mutationFn: () => createAssistantSession(),
    onSuccess: (response) => {
      setSelectedSessionId(response.session.id);
      void queryClient.invalidateQueries({ queryKey: ["assistant", "sessions"] });
      notify("success", t("assistant.sessionCreated"), { source: "assistant" });
    },
    onError: () => {
      notify("error", t("assistant.sessionCreateFailed"), { source: "assistant" });
    },
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, body }: { id: string; body: { title?: string; pinned?: boolean } }) =>
      updateAssistantSession(id, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["assistant", "sessions"] });
      if (selectedSessionId) {
        void queryClient.invalidateQueries({
          queryKey: ["assistant", "sessions", selectedSessionId, "state"],
        });
      }
    },
    onError: () => {
      notify("error", t("assistant.sessionUpdateFailed"), { source: "assistant" });
    },
  });

  const archiveMutation = useMutation({
    mutationFn: (id: string) => archiveAssistantSession(id),
    onSuccess: (_, id) => {
      if (selectedSessionId === id) setSelectedSessionId(null);
      void queryClient.invalidateQueries({ queryKey: ["assistant", "sessions"] });
      notify("success", t("assistant.sessionArchived"), { source: "assistant" });
    },
    onError: () => {
      notify("error", t("assistant.sessionArchiveFailed"), { source: "assistant" });
    },
  });

  const sendMutation = useMutation({
    mutationFn: ({ id, text }: { id: string; text: string }) => sendAssistantMessage(id, text),
    onSuccess: (_, { id }) => {
      void queryClient.invalidateQueries({ queryKey: ["assistant", "sessions"] });
      void queryClient.invalidateQueries({
        queryKey: ["assistant", "sessions", id, "state"],
      });
    },
    onError: () => {
      notify("error", t("assistant.messageSendFailed"), { source: "assistant" });
    },
  });

  const setOpen = useCallback(
    (open: boolean) => {
      setAssistantWindow((current) => ({ ...current, open }));
    },
    [setAssistantWindow],
  );

  const setDock = useCallback(
    (dock: AssistantWindowDock) => {
      setAssistantWindow((current) => ({ ...current, dock, open: true }));
    },
    [setAssistantWindow],
  );

  const setWindowSize = useCallback(
    (size: { width: number; height: number }) => {
      setAssistantWindow((current) =>
        normalizeAssistantWindowSetting({
          ...current,
          width: size.width,
          height: size.height,
        }),
      );
    },
    [setAssistantWindow],
  );

  const refresh = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ["assistant"] });
  }, [queryClient]);

  const value = useMemo<AssistantContextValue>(
    () => ({
      bootstrap: bootstrapQuery.data,
      bootstrapLoading: bootstrapQuery.isLoading,
      bootstrapError: bootstrapQuery.isError,
      assistantReady,
      canUseSessionShell,
      canUpdatePreferences,
      sessions,
      sessionsLoading: sessionsQuery.isLoading,
      sessionsFetching: sessionsQuery.isFetching,
      selectedSessionId,
      selectedSession,
      sessionState: sessionStateQuery.data,
      sessionStateLoading: sessionStateQuery.isLoading,
      sessionStateFetching: sessionStateQuery.isFetching,
      isCreatingSession: createMutation.isPending,
      isArchivingSession: archiveMutation.isPending,
      isSendingMessage: sendMutation.isPending,
      window: windowSetting.value,
      setOpen,
      setDock,
      setWindowSize,
      selectSession: setSelectedSessionId,
      createSession: async () => {
        try {
          await createMutation.mutateAsync();
        } catch {
          /* mutation onError already reports the failure */
        }
      },
      updateSession: async (id, body) => {
        try {
          await updateMutation.mutateAsync({ id, body });
        } catch {
          /* mutation onError already reports the failure */
        }
      },
      archiveSession: async (id) => {
        try {
          await archiveMutation.mutateAsync(id);
        } catch {
          /* mutation onError already reports the failure */
        }
      },
      sendMessage: async (id, text) => {
        try {
          await sendMutation.mutateAsync({ id, text });
        } catch {
          /* mutation onError already reports the failure */
        }
      },
      refresh,
    }),
    [
      archiveMutation,
      assistantReady,
      bootstrapQuery.data,
      bootstrapQuery.isError,
      bootstrapQuery.isLoading,
      canUseSessionShell,
      canUpdatePreferences,
      createMutation,
      refresh,
      selectedSession,
      selectedSessionId,
      sendMutation,
      sessionStateQuery.data,
      sessionStateQuery.isFetching,
      sessionStateQuery.isLoading,
      sessions,
      sessionsQuery.isFetching,
      sessionsQuery.isLoading,
      setDock,
      setOpen,
      setWindowSize,
      updateMutation,
      windowSetting.value,
    ],
  );

  return <AssistantContext.Provider value={value}>{children}</AssistantContext.Provider>;
}

