import { cn } from "@/shared/lib/utils";
import type { LucideIcon } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Plus } from "lucide-react";
import type { ReactNode } from "react";

interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  description?: string;
  actionLabel?: string;
  onAction?: () => void;
  actionIcon?: LucideIcon;
  children?: ReactNode;
  actions?: ReactNode;
  className?: string;
}

const EmptyState = ({ icon: Icon, title, description, actionLabel, onAction, actionIcon: ActionIcon = Plus, children, actions, className }: EmptyStateProps) => (
  <div className={cn("flex w-full flex-col items-center justify-center gap-2 px-4 py-12 text-center", className)}>
    <div className="mb-2 rounded-lg border border-border bg-muted p-4">
      <Icon className="size-6 text-muted-foreground" aria-hidden="true" />
    </div>
    <h3 className="text-sm font-semibold text-foreground">{title}</h3>
    {description && <p className="mx-auto max-w-sm text-sm text-muted-foreground leading-relaxed text-balance">{description}</p>}
    {children}
    {(actions || (actionLabel && onAction)) && (
      <div className="mt-2 flex flex-wrap items-center justify-center gap-2">
        {actionLabel && onAction && (
          <Button size="sm" onClick={onAction}>
            <ActionIcon className="size-3" aria-hidden="true" />
            {actionLabel}
          </Button>
        )}
        {actions}
      </div>
    )}
  </div>
);

export default EmptyState;
