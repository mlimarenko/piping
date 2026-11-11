import { useCallback, useEffect, useRef } from "react";
import { cn } from "@/shared/lib/utils";
import { beginPanelPointerResize } from "./panelPointerResize";

interface Props {
  /** "vertical" = drag vertically (handle is horizontal bar between rows) */
  /** "horizontal" = drag horizontally (handle is vertical bar between columns) */
  orientation: "vertical" | "horizontal";
  /** Current size of the leading panel in pixels */
  size: number;
  /** Minimum size (px) */
  min?: number;
  /** Maximum size (px) */
  max?: number;
  onResize: (newSize: number) => void;
  className?: string;
  reverse?: boolean;
  label?: string;
}

const PaneResizer = ({ orientation, size, min = 120, max = 2000, onResize, className, reverse = false, label }: Props) => {
  const resizeCleanup = useRef<(() => void) | null>(null);
  useEffect(() => () => resizeCleanup.current?.(), []);

  const onPointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || event.isPrimary === false) return;
    resizeCleanup.current?.();
    resizeCleanup.current = beginPanelPointerResize(event, orientation === "vertical" ? "y" : "x", delta => {
      onResize(Math.max(min, Math.min(max, size + delta * (reverse ? -1 : 1))));
    }, () => { resizeCleanup.current = null; });
  }, [orientation, size, min, max, onResize, reverse]);

  return (
    <div
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={size}
      aria-orientation={orientation === "horizontal" ? "vertical" : "horizontal"}
      onKeyDown={event => {
        const negative = orientation === "horizontal" ? "ArrowLeft" : "ArrowUp";
        const positive = orientation === "horizontal" ? "ArrowRight" : "ArrowDown";
        if (![negative, positive, "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const step = event.key === positive ? 20 : -20;
        const direction = reverse ? -1 : 1;
        let next: number;
        if (event.key === "Home") next = min;
        else if (event.key === "End") next = max;
        else next = size + step * direction;
        onResize(Math.max(min, Math.min(max, next)));
      }}
      onPointerDown={onPointerDown}
      className={cn(
        "shrink-0 flex items-center justify-center group touch-none transition-colors",
        orientation === "vertical"
          ? "h-1.5 w-full cursor-ns-resize hover:bg-primary/15 active:bg-primary/25"
          : "w-1.5 h-full cursor-ew-resize hover:bg-primary/15 active:bg-primary/25",
        className
      )}
    >
      <div className={cn(
        "rounded-full bg-border/60 group-hover:bg-primary/60 transition-colors",
        orientation === "vertical" ? "w-10 h-0.5" : "h-10 w-0.5"
      )} />
    </div>
  );
};

export default PaneResizer;
