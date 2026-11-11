import { toast as sonnerToast, type ExternalToast } from "sonner";

export type NotificationLevel = "info" | "success" | "warning" | "error";

/**
 * Show a toast notification — the single notification entry-point for app code.
 * Picks the sonner variant from `level` and forwards the rest to sonner.
 *
 * `source` is an optional call-site category tag (e.g. "connections"); it is
 * accepted for grouping intent but not otherwise rendered.
 */
export const notify = (
  level: NotificationLevel,
  title: string,
  options?: { description?: string; source?: string } & ExternalToast,
) => {
  const { source: _source, ...toastOpts } = options ?? {};
  const fn =
    level === "success"
      ? sonnerToast.success
      : level === "error"
        ? sonnerToast.error
        : level === "warning"
          ? sonnerToast.warning
          : sonnerToast.info;
  fn(title, toastOpts);
};
