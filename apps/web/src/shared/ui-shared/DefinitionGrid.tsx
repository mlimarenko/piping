import type { ReactNode } from "react";

/** A read-only definition-list grid: one label/value pair per field. */
export interface DefinitionGridField {
  label: string;
  value: ReactNode;
}

interface DefinitionGridProps {
  fields: DefinitionGridField[];
}

/**
 * Pure read-only field composition: a responsive `dl` grid where each field is
 * a `dt` label over a wrapping `dd` value. Owns no fetch, capability or status
 * logic — callers pass already-computed label/value pairs (including any status
 * chip rendered as a field).
 */
const DefinitionGrid = ({ fields }: DefinitionGridProps) => (
  <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
    {fields.map((field) => (
      <div key={field.label}>
        <dt className="text-sm text-muted-foreground">{field.label}</dt>
        <dd className="mt-1 break-words font-medium">{field.value || "—"}</dd>
      </div>
    ))}
  </dl>
);

export default DefinitionGrid;
