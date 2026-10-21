import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";

export interface FormSectionProps {
  title?: ReactNode;
  description?: ReactNode;
  /** Right-aligned slot on the title row (count badge + clear-all, Add button…). */
  action?: ReactNode;
  /** Appends the shared required asterisk to the title. */
  required?: boolean;
  /** Adds space before the next related group of fields. */
  divider?: boolean;
  className?: string;
  contentClassName?: string;
  children: ReactNode;
}

/** A shared field group with a readable heading and a quiet inset surface. */
const FormSection = ({
  title,
  description,
  action,
  required,
  divider,
  className,
  contentClassName,
  children,
}: FormSectionProps) => {
  const { t } = useTranslation();

  return (
    <section data-slot="form-section" className={cn("flex min-w-0 flex-col gap-4 rounded-lg bg-muted/45 p-4 md:p-6", divider && "mt-2", className)}>
      {(title || description || action) && (
        <div data-slot="form-section-header" className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-1">
            {title ? (
              <h2 className="text-base font-semibold tracking-tight text-foreground">
                {title}
                {required && (
                  <>
                    <span aria-hidden="true" className="ml-0.5 text-destructive">
                      *
                    </span>
                    <span className="sr-only"> ({t("required")})</span>
                  </>
                )}
              </h2>
            ) : null}
            {description ? (
              <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">{description}</p>
            ) : null}
          </div>
          {action ? <div className="flex shrink-0 items-center gap-2">{action}</div> : null}
        </div>
      )}
       <div data-slot="form-section-content" className={cn("min-w-0", contentClassName)}>{children}</div>
    </section>
  );
};

export default FormSection;
