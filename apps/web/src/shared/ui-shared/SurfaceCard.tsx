import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "@/shared/lib/utils";

export interface SurfaceCardProps extends HTMLAttributes<HTMLDivElement> {
  children: ReactNode;
  variant?: "default" | "inset";
  hover?: boolean;
  selected?: boolean;
}

const SurfaceCard = ({ children, className, variant = "default", hover, selected, ...props }: SurfaceCardProps) => {
  const variantClass = {
    default: "surface",
    inset: "surface-inset",
  }[variant];

  return (
    <div
      data-slot="surface-card"
      className={cn(
        variantClass,
        "rounded-lg",
        hover && "transition-colors hover:border-primary/40",
        selected && "selection-ring",
        className,
      )}
      {...props}
    >
      {children}
    </div>
  );
};

export interface SurfaceCardHeaderProps extends Omit<HTMLAttributes<HTMLDivElement>, "title"> {
  title?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  headingLevel?: 2 | 3 | 4;
}

export const SurfaceCardHeader = ({
  title,
  description,
  actions,
  headingLevel = 2,
  children,
  className,
  ...props
}: SurfaceCardHeaderProps) => {
  const Heading: "h2" | "h3" | "h4" = `h${headingLevel}`;

  return (
    <div
      data-slot="surface-card-header"
      className={cn("flex flex-wrap items-start justify-between gap-4 border-b border-border p-4 md:p-6", className)}
      {...props}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        {title != null && (
          <Heading data-slot="surface-card-title" className="text-base font-semibold tracking-tight text-foreground">
            {title}
          </Heading>
        )}
        {description != null && (
          <p data-slot="surface-card-description" className="text-sm leading-relaxed text-muted-foreground">
            {description}
          </p>
        )}
        {children}
      </div>
      {actions != null && (
        <div data-slot="surface-card-actions" className="flex shrink-0 items-center gap-2">
          {actions}
        </div>
      )}
    </div>
  );
};

export const SurfaceCardContent = ({ className, ...props }: HTMLAttributes<HTMLDivElement>) => (
  <div data-slot="surface-card-content" className={cn("min-w-0 p-4 md:p-6", className)} {...props} />
);

export default SurfaceCard;
