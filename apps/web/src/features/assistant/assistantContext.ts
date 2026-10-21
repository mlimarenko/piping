import { createContext, useContext } from "react";

import type {
  AssistantBootstrap,
  AssistantSession,
  AssistantSessionState,
} from "./api";
import type {
  AssistantWindowDock,
  AssistantWindowSettingValue,
} from "@/shared/settings/registry";

export interface AssistantContextValue {
  bootstrap?: AssistantBootstrap;
  bootstrapLoading: boolean;
  bootstrapError: boolean;
  /** True only when bootstrap is loaded, assistant is configured, and the user has Execute permission. */
  assistantReady: boolean;
  canUseSessionShell: boolean;
  canUpdatePreferences: boolean;
  sessions: AssistantSession[];
  sessionsLoading: boolean;
  sessionsFetching: boolean;
  selectedSessionId: string | null;
  selectedSession?: AssistantSession;
  sessionState?: AssistantSessionState;
  sessionStateLoading: boolean;
  sessionStateFetching: boolean;
  isCreatingSession: boolean;
  isArchivingSession: boolean;
  isSendingMessage: boolean;
  window: AssistantWindowSettingValue;
  setOpen: (open: boolean) => void;
  setDock: (dock: AssistantWindowDock) => void;
  setWindowSize: (size: { width: number; height: number }) => void;
  selectSession: (id: string | null) => void;
  createSession: () => Promise<void>;
  updateSession: (id: string, body: { title?: string; pinned?: boolean }) => Promise<void>;
  archiveSession: (id: string) => Promise<void>;
  sendMessage: (id: string, text: string) => Promise<void>;
  refresh: () => void;
}

export const AssistantContext = createContext<AssistantContextValue | null>(null);

export function useAssistant() {
  const value = useContext(AssistantContext);
  if (!value) {
    throw new Error("useAssistant must be used inside AssistantProvider");
  }
  return value;
}
