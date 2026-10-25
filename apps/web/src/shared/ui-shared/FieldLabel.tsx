import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Label } from "@/shared/ui-primitives/label";
import { cn } from "@/shared/lib/utils";

export interface FieldLabelProps extends ComponentPropsWithoutRef<typeof Label> {
  /** Marks the field as required: appends a red asterisk + an a11y hint. */
  required?: boolean;
  children: ReactNode;
}

/**
 * Unified field label for forms across the app. Wraps the shared Radix
 * `Label` primitive (so it keeps `htmlFor` association + `peer-disabled`
 * styling) and, when `required`, appends a red asterisk (`*`) plus a
 * localized screen-reader-only required hint.
 *
 * The label row accommodates the shared 32px help action even when no action
 * is present, so adjacent controls align. Longer labels wrap without clipping.
 */
function FieldLabel({
  required,
  className,
  children,
  ...rest
}: FieldLabelProps) {
  const { t } = useTranslation();

  return (
    <Label className={cn("inline-flex min-h-8 min-w-0 items-center gap-1 leading-5 [overflow-wrap:anywhere]", className)} {...rest}>
      {children}
      {required && (
        <>
          <span aria-hidden="true" className="ml-0.5 text-destructive">
            *
          </span>
          <span className="sr-only"> ({t("required")})</span>
        </>
      )}
    </Label>
  );
}

export default FieldLabel;
