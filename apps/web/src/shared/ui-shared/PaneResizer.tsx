import { useCallback, useRef } from "react";
import { cn } from "@/shared/lib/utils";

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
  const startPos = useRef(0);
  const startSize = useRef(0);

  const onMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    startPos.current = orientation === "vertical" ? e.clientY : e.clientX;
    startSize.current = size;

    const onMove = (ev: MouseEvent) => {
      const cur = orientation === "vertical" ? ev.clientY : ev.clientX;
      const delta = cur - startPos.current;
      onResize(Math.max(min, Math.min(max, startSize.current + delta * (reverse ? -1 : 1))));
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = orientation === "vertical" ? "ns-resize" : "ew-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, [orientation, size, min, max, onResize, reverse]);

  const onTouchStart = useCallback((e: React.TouchEvent) => {
    const t = e.touches[0];
    if (!t) return;
    startPos.current = orientation === "vertical" ? t.clientY : t.clientX;
    startSize.current = size;

    const onMove = (ev: TouchEvent) => {
      const tt = ev.touches[0];
      if (!tt) return;
      const cur = orientation === "vertical" ? tt.clientY : tt.clientX;
      const delta = cur - startPos.current;
      onResize(Math.max(min, Math.min(max, startSize.current + delta * (reverse ? -1 : 1))));
    };
    const onEnd = () => {
      document.removeEventListener("touchmove", onMove);
      document.removeEventListener("touchend", onEnd);
    };
    document.addEventListener("touchmove", onMove, { passive: true });
    document.addEventListener("touchend", onEnd);
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
      onMouseDown={onMouseDown}
      onTouchStart={onTouchStart}
      className={cn(
        "shrink-0 flex items-center justify-center group transition-colors",
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
