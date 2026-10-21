import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";

export interface UnsavedChangesGuard {
  /** Set true immediately before an intentional exit so the guards stand down. */
  allowExit: RefObject<boolean>;
  /** Open the discard confirmation without running the action now. */
  confirmExit: (action: () => void) => void;
  /** Discard-confirmation dialog; render once per guarded page. */
  discardDialog: ReactNode;
}

/**
 * Canonical unsaved-changes guard shared by the page-editor shells.
 *
 * While `dirty`, it blocks tab close/reload with `beforeunload` and intercepts
 * in-app `<a href>` navigation, routing both through the same discard
 * confirmation used by explicit Back/Cancel actions. A full-page navigation
 * cannot be intercepted without a data router, so `beforeunload` is the only
 * reliable tab-level guard.
 */
export function useUnsavedChangesGuard(dirty: boolean, saving = false): UnsavedChangesGuard {
  const { t } = useTranslation();
  const [exitAction, setExitAction] = useState<(() => void) | null>(null);
  const allowExit = useRef(false);

  const confirmExit = (action: () => void) => { if (!saving) setExitAction(() => action); };

  useEffect(() => {
    if (!dirty && !saving) return;
    const onNavigate = (event: MouseEvent) => {
      if (allowExit.current || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
      if (!link || link.target === "_blank" || link.hasAttribute("download")) return;
      const url = new URL(link.href);
      if (url.origin !== location.origin || (url.pathname === location.pathname && url.search === location.search)) return;
      event.preventDefault(); event.stopImmediatePropagation();
      if (saving) return;
      setExitAction(() => () => link.click());
    };
    document.addEventListener("click", onNavigate, true);
    return () => document.removeEventListener("click", onNavigate, true);
  }, [dirty, saving]);

  useEffect(() => {
    if (!dirty && !saving) return;
    const handler = (event: BeforeUnloadEvent) => { event.preventDefault(); };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [dirty, saving]);

  const discardDialog = (
    <AlertDialog open={!saving && exitAction !== null} onOpenChange={open => { if (!open) setExitAction(null); }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("entityEditor.discardTitle")}</AlertDialogTitle>
          <AlertDialogDescription>{t("entityEditor.discardDescription")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("entityEditor.continueEditing")}</AlertDialogCancel>
          <AlertDialogAction onClick={() => {
            if (saving) return;
            const action = exitAction;
            setExitAction(null);
            allowExit.current = true;
            action?.();
            allowExit.current = false;
          }}>{t("entityEditor.discard")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );

  return { allowExit, confirmExit, discardDialog };
}
