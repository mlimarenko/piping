import { useCallback, useEffect, useRef } from "react";

/** Keep menu-to-editor transitions anchored to the row, including row removal. */
export function useInventoryRowActionFocus() {
  const triggers = useRef(new Map<string, HTMLButtonElement>());
  const invoker = useRef<HTMLButtonElement | null>(null);
  const restoreFocus = useCallback(() => {
    const target = invoker.current?.isConnected ? invoker.current
      : triggers.current.values().next().value
        ?? document.querySelector<HTMLButtonElement>('[data-slot="page-actions"] button:not([disabled])');
    target?.focus();
  }, []);
  useEffect(() => {
    // A completed deletion can remove the invoker after the dialog has closed.
    // Do not steal focus if the operator has already moved to another control.
    if (invoker.current && !invoker.current.isConnected && document.activeElement === document.body) {
      restoreFocus();
      invoker.current = null;
    }
  });
  const triggerProps = useCallback((id: string) => ({
    ref: (node: HTMLButtonElement | null) => {
      if (node) triggers.current.set(id, node);
      else triggers.current.delete(id);
    },
    onOpenChange: (open: boolean) => {
      if (open) invoker.current = triggers.current.get(id) ?? null;
    },
  }), []);
  return { triggerProps, restoreFocus };
}
