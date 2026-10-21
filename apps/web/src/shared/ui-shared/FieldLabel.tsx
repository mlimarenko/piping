import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Label } from "@/shared/ui-primitives/label";

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
 * The asterisk is the single shared signal for "this field must be filled".
 * `className` is passed through untouched so each form keeps its own
 * sizing/colour; the asterisk uses `ml-0.5` so it renders correctly whether
 * the label is `block`, `inline`, or `inline-flex`.
 */
function FieldLabel({
  required,
  className,
  children,
  ...rest
}: FieldLabelProps) {
  const { t } = useTranslation();

  return (
    <Label className={className} {...rest}>
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
