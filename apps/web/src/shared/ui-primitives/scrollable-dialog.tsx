import type { ReactNode } from "react";
import { cn } from "@/shared/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";

interface ScrollableDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
  footer: ReactNode;
  children: ReactNode;
  className?: string;
}

/**
 * Dialog wrapper for long forms. The header and footer are sticky (do not
 * scroll); the body is the single scroll region. The `min-h-0` on the body is
 * mandatory — without it the flex child refuses to shrink and the overflow
 * never engages.
 *
 * Positioning override (`top-[7dvh] translate-y-0 transform-none`): the base
 * shadcn `DialogContent` centers with `top-50% translate-y-[-50%]`. Under
 * Tailwind v4 the `translate-*` utilities emit the CSS `translate` property
 * while `tailwindcss-animate` still drives a `-50%` via the `transform`
 * property — the two stack to `-100%`, riding a dialog taller than ~half the
 * viewport off the top edge (header/title clipped). We pin the dialog a fixed
 * `7dvh` from the top and zero BOTH vertical contributors, so a long form is
 * always anchored on-screen and the body scrolls within `max-h`.
 */
const ScrollableDialog = ({
  open,
  onOpenChange,
  title,
  description,
  footer,
  children,
  className,
}: ScrollableDialogProps) => (
  <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent
      className={cn(
        "flex max-h-[86dvh] flex-col gap-0 overflow-hidden p-0 sm:max-w-lg",
        "top-[7dvh] translate-y-0 transform-none",
        className,
      )}
    >
      <DialogHeader className="shrink-0 border-b border-border px-4 py-4 pr-10 sm:px-6 sm:pr-10">
        <DialogTitle>{title}</DialogTitle>
        {description ? <DialogDescription>{description}</DialogDescription> : null}
      </DialogHeader>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">{children}</div>

      <DialogFooter className="shrink-0 border-t border-border px-4 py-4 sm:px-6">
        {footer}
      </DialogFooter>
    </DialogContent>
  </Dialog>
);

export default ScrollableDialog;
