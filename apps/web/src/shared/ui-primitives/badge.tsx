import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/shared/lib/utils";

const badgeVariants = cva(
  "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground hover:bg-primary/80",
        secondary: "border-transparent bg-secondary text-secondary-foreground hover:bg-secondary/80",
        destructive: "border-transparent bg-destructive text-destructive-foreground hover:bg-destructive/80",
        outline: "text-foreground",
        // Status variants — colour only; caller is responsible for i18n text
        "status-active":         "border-status-positive/20  bg-status-positive/10  text-status-positive",
        "status-planned":        "border-status-info/20      bg-status-info/10      text-status-info",
        "status-offline":        "border-status-negative/20  bg-status-negative/10  text-status-negative",
        "status-decommissioning":"border-status-negative/20  bg-status-negative/10  text-status-negative",
        "status-staging":        "border-status-warning/20   bg-status-warning/10   text-status-warning",
        "status-failed":         "border-status-negative/20  bg-status-negative/10  text-status-negative",
        "status-neutral":        "border-border              bg-muted/40            text-muted-foreground",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

export interface BadgeProps extends React.HTMLAttributes<HTMLDivElement>, VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return <div className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { Badge, badgeVariants };
