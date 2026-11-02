import { normalizeHex } from "@/shared/forms/colorUtils";

/** Keep arbitrary catalog CSS out of the shared label projection. */
export default function EntityOptionLabel({ label, color }: { label: string; color?: string }) {
  const normalized = color ? normalizeHex(color) : null;
  return <span className="inline-flex min-w-0 items-center gap-1.5">
    {normalized && <span aria-hidden="true" className="size-3 shrink-0 rounded-sm border border-border" style={{ backgroundColor: normalized }} />}
    <span className="break-words [overflow-wrap:anywhere]">{label}</span>
  </span>;
}
