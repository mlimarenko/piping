import {
  Archive,
  Bot,
  ChevronDown,
  Loader2,
  Maximize2,
  PanelBottom,
  PanelLeft,
  PanelRight,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  SendHorizontal,
  X,
} from "lucide-react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";

import { useAssistant } from "./assistantContext";
import type { AssistantMessage } from "./api";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { cn } from "@/shared/lib/utils";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { ScrollArea } from "@/shared/ui-primitives/scroll-area";
import { Textarea } from "@/shared/ui-primitives/textarea";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";
import type { AssistantWindowDock } from "@/shared/settings/registry";

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, Math.round(value)));
}

function messageText(message: AssistantMessage): string {
  const content = message.content_json;
  if (typeof content === "string") return content;
  if (content && typeof content === "object" && !Array.isArray(content)) {
    const record = content as Record<string, unknown>;
    for (const key of ["text", "content", "message", "summary"]) {
      const value = record[key];
      if (typeof value === "string") return value;
    }
  }
  try {
    return JSON.stringify(content, null, 2);
  } catch {
    return "";
  }
}

function dockClass(dock: AssistantWindowDock): string {
  if (dock === "left") return "left-16 top-14 bottom-8 resize-x";
  if (dock === "bottom") return "left-16 right-4 bottom-8 resize-y";
  if (dock === "floating") return "right-5 bottom-12 resize";
  return "right-4 top-14 bottom-8 resize-x";
}

function dockStyle(dock: AssistantWindowDock, width: number, height: number): CSSProperties {
  if (dock === "bottom") {
    return { height: clamp(height, 280, 720) };
  }
  if (dock === "floating") {
    return {
      width: clamp(width, 320, 900),
      height: clamp(height, 360, 860),
    };
  }
  return { width: clamp(width, 340, 760) };
}

function DockButton({
  dock,
  activeDock,
  onClick,
  label,
  children,
}: {
  dock: AssistantWindowDock;
  activeDock: AssistantWindowDock;
  onClick: (dock: AssistantWindowDock) => void;
  label: string;
  children: ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant={activeDock === dock ? "secondary" : "ghost"}
          size="icon-sm"
          className="h-8 w-8"
          aria-label={label}
          onClick={() => onClick(dock)}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent className="text-sm">{label}</TooltipContent>
    </Tooltip>
  );
}

function statusVariant(status: string) {
  if (status === "session_shell_ready") return "status-active";
  if (status === "disabled_by_instance") return "status-offline";
  return "status-neutral";
}

export function AssistantWindow() {
  const { t } = useTranslation();
  const assistant = useAssistant();
  const { can } = usePermissions();
  const panelRef = useRef<HTMLDivElement | null>(null);
  const resizeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [draft, setDraft] = useState("");
  const { dock, width, height, open } = assistant.window;
  const setWindowSize = assistant.setWindowSize;

  useEffect(() => {
    if (!open || !panelRef.current) return;
    const element = panelRef.current;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const nextWidth = clamp(entry.contentRect.width, 320, 900);
      const nextHeight = clamp(entry.contentRect.height, 280, 860);
      if (
        Math.abs(nextWidth - width) < 8 &&
        Math.abs(nextHeight - height) < 8
      ) {
        return;
      }
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
      resizeTimer.current = setTimeout(() => {
        setWindowSize({ width: nextWidth, height: nextHeight });
      }, 250);
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      if (resizeTimer.current) clearTimeout(resizeTimer.current);
    };
  }, [height, open, setWindowSize, width]);

  const messages = assistant.sessionState?.messages ?? [];
  const status = assistant.bootstrap?.feature_status ?? "loading";
  const provider = assistant.bootstrap?.config?.provider ?? t("assistant.providerUnset");
  const model = assistant.bootstrap?.config?.model ?? t("assistant.modelUnset");
  const selected = assistant.selectedSession;
  const canSendMessages = Boolean(assistant.bootstrap?.capabilities?.chat_messages);
  const isSessionRunning = selected?.status === "running";
  const draftIsEmpty = draft.trim().length === 0;
  const sendDisabled =
    !selected ||
    !canSendMessages ||
    assistant.isSendingMessage ||
    isSessionRunning ||
    draftIsEmpty;
  const inputPlaceholder = !selected
    ? t("assistant.inputNoSession")
    : !canSendMessages
      ? t("assistant.inputChatUnavailable")
      : t("assistant.inputPlaceholder");

  const submitMessage = () => {
    if (!selected || sendDisabled) return;
    const text = draft.trim();
    setDraft("");
    void assistant.sendMessage(selected.id, text);
  };

  const activeSessions = useMemo(
    () => assistant.sessions.filter((session) => session.lifecycle === "active"),
    [assistant.sessions],
  );

  if (!open || !assistant.assistantReady) return null;

  return (
    <TooltipProvider delayDuration={200}>
      <section
        ref={panelRef}
        className={cn(
          "bg-card fixed z-50 flex min-h-72 min-w-80 flex-col overflow-hidden rounded-lg border border-border/50",
          dockClass(dock),
        )}
        style={dockStyle(dock, width, height)}
        aria-label={t("assistant.windowTitle")}
      >
        <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border/35 px-3">
          <Bot className="h-4 w-4 shrink-0 text-primary" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold text-foreground">
              {t("assistant.windowTitle")}
            </div>
          </div>
          <Badge
            variant={statusVariant(status)}
            className="hidden h-5 shrink-0 px-1.5 text-xs font-medium sm:inline-flex"
          >
            {t(`assistant.status.${status}`, { defaultValue: status })}
          </Badge>
          <DockButton
            dock="left"
            activeDock={dock}
            onClick={assistant.setDock}
            label={t("assistant.dockLeft")}
          >
            <PanelLeft className="h-3.5 w-3.5" />
          </DockButton>
          <DockButton
            dock="right"
            activeDock={dock}
            onClick={assistant.setDock}
            label={t("assistant.dockRight")}
          >
            <PanelRight className="h-3.5 w-3.5" />
          </DockButton>
          <DockButton
            dock="bottom"
            activeDock={dock}
            onClick={assistant.setDock}
            label={t("assistant.dockBottom")}
          >
            <PanelBottom className="h-3.5 w-3.5" />
          </DockButton>
          <DockButton
            dock="floating"
            activeDock={dock}
            onClick={assistant.setDock}
            label={t("assistant.dockFloating")}
          >
            <Maximize2 className="h-3.5 w-3.5" />
          </DockButton>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="h-8 w-8"
            aria-label={t("refresh")}
            onClick={assistant.refresh}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", assistant.sessionsFetching && "animate-spin")} />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="h-8 w-8"
            aria-label={t("close")}
            onClick={() => assistant.setOpen(false)}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        </header>

        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex shrink-0 items-center gap-2 border-b border-border/25 px-3 py-2">
            <ScrollArea className="min-w-0 flex-1 whitespace-nowrap">
              <div className="flex gap-1.5 pr-2">
                {activeSessions.map((session) => (
                  <Button variant="ghost" size="sm"
                    key={session.id}
                    type="button"
                    className={cn(
                      "h-auto min-h-9 max-w-44 shrink-0 flex-col items-start gap-0 rounded-md border px-3 py-2 text-left text-sm transition-colors",
                      assistant.selectedSessionId === session.id
                        ? "border-border bg-accent text-primary"
                        : "border-input/30 bg-muted text-muted-foreground hover:bg-accent hover:text-foreground",
                    )}
                    onClick={() => assistant.selectSession(session.id)}
                  >
                    <span className="block w-full min-w-0 truncate font-medium" title={session.title}>{session.title}</span>
                    <span className="block w-full min-w-0 truncate text-xs opacity-70">
                      {session.message_count} · {session.status}
                    </span>
                  </Button>
                ))}
              </div>
            </ScrollArea>
            {can("EXECUTE", "ASSISTANT") && (
              <Button
                type="button"
                size="icon-sm"
                className="h-8 w-8 shrink-0"
                disabled={!assistant.canUseSessionShell || assistant.isCreatingSession}
                aria-label={t("assistant.newSession")}
                onClick={() => {
                  void assistant.createSession();
                }}
              >
                {assistant.isCreatingSession ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Plus className="h-3.5 w-3.5" />
                )}
              </Button>
            )}
            {selected && (
              <>
                {can("UPDATE", "ASSISTANT") && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="h-8 w-8 shrink-0"
                    disabled={!assistant.canUpdatePreferences}
                    aria-label={selected.pinned ? t("assistant.unpin") : t("assistant.pin")}
                    onClick={() => {
                      void assistant.updateSession(selected.id, { pinned: !selected.pinned });
                    }}
                  >
                    {selected.pinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
                  </Button>
                )}
                {can("UPDATE", "ASSISTANT") && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="h-8 w-8 shrink-0 text-destructive hover:text-destructive"
                    disabled={assistant.isArchivingSession || selected.status === "running"}
                    aria-label={t("assistant.archiveSession")}
                    onClick={() => {
                      void assistant.archiveSession(selected.id);
                    }}
                  >
                    <Archive className="h-3.5 w-3.5" />
                  </Button>
                )}
              </>
            )}
          </div>

          <main className="flex min-h-0 flex-1 flex-col">
            {!assistant.canUseSessionShell ? (
              <div className="flex min-h-0 flex-1 items-center justify-center px-6 text-center">
                <div className="space-y-2">
                  <Badge variant={statusVariant(status)} className="mx-auto">
                    {t(`assistant.status.${status}`, { defaultValue: status })}
                  </Badge>
                  <p className="text-sm text-muted-foreground">
                    {t(`assistant.statusDetails.${status}`, { defaultValue: "" })}
                  </p>
                </div>
              </div>
            ) : assistant.sessionsLoading ? (
              <div className="flex min-h-0 flex-1 items-center justify-center text-sm text-muted-foreground">
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                {t("loading")}
              </div>
            ) : !selected ? (
              <div className="flex min-h-0 flex-1 items-center justify-center">
                {can("EXECUTE", "ASSISTANT") ? (
                  <Button
                    type="button"
                    size="sm"
                    className="h-9 gap-1 text-sm"
                    disabled={assistant.isCreatingSession}
                    onClick={() => {
                      void assistant.createSession();
                    }}
                  >
                    <Plus className="h-3.5 w-3.5" />
                    {t("assistant.newSession")}
                  </Button>
                ) : (
                  <p className="px-6 text-center text-sm text-muted-foreground">
                    {t("assistant.emptyTranscript")}
                  </p>
                )}
              </div>
            ) : (
              <>
                <ScrollArea className="min-h-0 flex-1">
                  <div className="space-y-2 p-3">
                    {assistant.sessionStateLoading ? (
                      <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        {t("loading")}
                      </div>
                    ) : messages.length === 0 ? (
                      <div className="rounded-md border border-dashed border-border/40 px-3 py-6 text-center text-sm text-muted-foreground">
                        {t("assistant.emptyTranscript")}
                      </div>
                    ) : (
                      messages.map((message) => (
                        <article
                          key={message.id}
                          className={cn(
                            "rounded-md border px-3 py-2 text-sm",
                            message.role === "user"
                              ? "ml-8 border-border bg-accent"
                              : "mr-8 border-border/35 bg-muted",
                          )}
                        >
                          <div className="mb-1 flex items-center gap-2 text-xs uppercase text-muted-foreground">
                            <span>{message.role}</span>
                            <span>{new Date(message.created_at).toLocaleString()}</span>
                          </div>
                          <pre className="whitespace-pre-wrap break-words font-sans text-sm leading-5 text-foreground">
                            {messageText(message)}
                          </pre>
                        </article>
                      ))
                    )}
                  </div>
                </ScrollArea>
                <div className="shrink-0 border-t border-border/25 p-3">
                  <form
                    className="flex items-end gap-2"
                    onSubmit={(event) => {
                      event.preventDefault();
                      submitMessage();
                    }}
                  >
                    <Textarea
                      disabled={
                        !selected ||
                        !canSendMessages ||
                        assistant.isSendingMessage ||
                        isSessionRunning
                      }
                      rows={2}
                      value={draft}
                      placeholder={inputPlaceholder}
                      className="min-h-16 resize-none text-sm"
                      onChange={(event) => setDraft(event.target.value)}
                      onKeyDown={(event) => {
                        if (event.key !== "Enter" || event.shiftKey) return;
                        event.preventDefault();
                        submitMessage();
                      }}
                    />
                    <Button
                      type="submit"
                      size="icon-sm"
                      className="h-9 w-9 shrink-0"
                      disabled={sendDisabled}
                      aria-label={t("assistant.sendMessage")}
                    >
                      {assistant.isSendingMessage || isSessionRunning ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <SendHorizontal className="h-3.5 w-3.5" />
                      )}
                    </Button>
                  </form>
                </div>
              </>
            )}
          </main>
        </div>

        <footer className="flex h-9 shrink-0 items-center gap-2 border-t border-border/25 px-3 text-xs text-muted-foreground">
          {assistant.sessionStateFetching && <Loader2 className="h-3 w-3 animate-spin" />}
          <span className="truncate">{provider}</span>
          <ChevronDown className="h-3 w-3 rotate-[-90deg]" />
          <span className="truncate">{model}</span>
        </footer>
      </section>
    </TooltipProvider>
  );
}
