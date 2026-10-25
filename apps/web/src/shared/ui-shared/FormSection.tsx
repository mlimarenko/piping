import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "./SurfaceCard";

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

/** Form groups use the same surface and header as other page sections. */
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
    <SurfaceCard data-slot="form-section" className={cn("min-w-0", divider && "mt-2", className)}>
      {(title || description || action) && (
        <SurfaceCardHeader data-slot="form-section-header" description={description} actions={action}
          title={title ? <>
                {title}
                {required && (
                  <>
                    <span aria-hidden="true" className="ml-0.5 text-destructive">
                      *
                    </span>
                    <span className="sr-only"> ({t("required")})</span>
                  </>
                )}
          </> : undefined} />
      )}
      <SurfaceCardContent data-slot="form-section-content" className={contentClassName}>{children}</SurfaceCardContent>
    </SurfaceCard>
  );
};

export default FormSection;
