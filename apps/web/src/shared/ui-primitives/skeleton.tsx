import { cn } from "@/shared/lib/utils";

/**
 * Skeleton — uses a shimmer sweep instead of a flat pulse so it reads clearly
 * against the shared card surfaces used throughout the UI. The gradient rides
 * the same cool-tint palette as the surface system.
 */
function Skeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-md",
        "bg-[hsl(var(--surface-color)/0.55)]",
        "after:absolute after:inset-0",
        "after:bg-[linear-gradient(90deg,transparent_0%,hsl(var(--surface-color)/0.80)_50%,transparent_100%)]",
        "after:animate-[skeleton-shimmer_1.6s_ease-in-out_infinite]",
        className,
      )}
      {...props}
    />
  );
}

export { Skeleton };
