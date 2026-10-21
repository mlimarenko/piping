import type { LucideIcon } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { UUID_RE } from "@/shared/lib/ids";

const shortId = (id: string) => (id.length > 12 ? `${id.slice(0, 8)}…` : id);

export interface EntityRefProps {
  /** Resolved human-readable name. When present it is shown as the label. */
  name?: string | null;
  /** Raw id (usually a UUID). Surfaced via the title tooltip and used for the fallback. */
  id?: string | null;
  /** Optional type icon (lucide). */
  icon?: LucideIcon;
  className?: string;
}

/**
 * Renders an entity reference as a friendly label with an optional type icon.
 *
 * It never shows a bare UUID to the user: when `name` is missing it falls back
 * to a muted, monospace short-id (`abcd1234…`) and exposes the full id through
 * the native title tooltip — so a deleted/loading entity reads as clearly
 * "unknown" instead of an inscrutable raw UUID. When the value isn't a UUID
 * (e.g. it's already a name), it is shown verbatim.
 */
function EntityRef({ name, id, icon: Icon, className }: EntityRefProps) {
  const resolved = typeof name === "string" && name.trim().length > 0;
  const looksLikeId = !!id && UUID_RE.test(id);

  const label = resolved ? name : id ? (looksLikeId ? shortId(id) : id) : "—";
  // Tooltip: full id when it adds information (unresolved id, or name+id pair).
  const title = id
    ? resolved
      ? `${name} · ${id}`
      : looksLikeId
        ? id
        : undefined
    : undefined;

  return (
    <span
      className={cn("inline-flex min-w-0 items-center gap-1.5", className)}
      title={title}
    >
      {Icon && (
        <Icon
          className={cn(
            "h-3 w-3 shrink-0",
            resolved ? "text-muted-foreground" : "text-muted-foreground/50",
          )}
        />
      )}
      <span
        className={cn(
          "min-w-0 truncate",
          resolved ? "" : "font-mono text-[0.95em] text-muted-foreground/70",
        )}
      >
        {label}
      </span>
    </span>
  );
}

export default EntityRef;
