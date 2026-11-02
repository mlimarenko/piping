import { useTranslation } from "react-i18next";
import type { InventoryEntityKey } from "@/shared/api/types";
import type { InventoryResourceOption } from "@/features/inventory/resourceOptions";
import { Badge } from "@/shared/ui-primitives/badge";
import { formatTemplateValue, TEMPLATE_RESOURCE_DEFINITION_MAP, TEMPLATE_REFERENCE_ENTITIES, templatePayloadFieldLabel, type PreviewRow, type TemplatePayloadFieldDefinition } from "./templateSchema";

interface Props {
  rows: PreviewRow[];
  optionsByEntity?: ReadonlyMap<InventoryEntityKey, { options: InventoryResourceOption[] }>;
}

const objectName = (row: PreviewRow) => formatTemplateValue(row.payload.name ?? row.payload.address ?? row.payload.slug) || row.title;

export default function TemplatePreviewRows({ rows, optionsByEntity }: Props) {
  const { t } = useTranslation();
  const displayValue = (value: unknown, field: TemplatePayloadFieldDefinition): string => {
    if (Array.isArray(value)) return value.map((item) => displayValue(item, field)).join(", ");
    const target = field.references ?? field.referencesByName;
    if (target && typeof value === "string") {
      const future = /^preview:(.+):(-?\d+)$/.exec(value);
      if (future) {
        const candidates = rows.filter((row) => row.resource_type === target && (row.step_key === future[1] || row.step_key.startsWith(`${future[1]}[`)));
        const row = candidates.find((candidate) => candidate.step_key === `${future[1]}[${future[2]}]`)
          ?? (candidates.length === 1 && candidates[0]?.step_key === future[1] ? candidates[0] : undefined);
        if (row) return t("inventory.templates.run.futureObject", { name: objectName(row), step: rows.indexOf(row) + 1 });
      }
      const option = optionsByEntity?.get(TEMPLATE_REFERENCE_ENTITIES[target])?.options.find((item) => item.id === value);
      if (option) return [option.name, option.subtitle].filter(Boolean).join(" · ");
    }
    return typeof value === "boolean" ? t(value ? "yes" : "no") : formatTemplateValue(value);
  };

  return <ol className="flex min-w-0 flex-col gap-2" aria-label={t("inventory.templates.run.previewHeading")}>
    {rows.map((row, index) => {
      const definition = TEMPLATE_RESOURCE_DEFINITION_MAP[row.resource_type];
      const entries = Object.entries(row.payload).filter(([, value]) => value !== undefined && value !== null && value !== "");
      const identityKey = row.payload.name != null ? "name" : row.payload.address != null ? "address" : row.payload.slug != null ? "slug" : null;
      const known = entries.flatMap(([key, value]) => {
        if (key === identityKey) return [];
        const field = definition?.payloadFields.find((candidate) => candidate.key === key);
        return field ? [{ field, value }] : [];
      });
      const unknown = entries.filter(([key]) => !definition?.payloadFields.some((field) => field.key === key));
      return <li key={`${row.step_key}-${index}`} className="surface-inset flex min-w-0 items-start gap-2 rounded-md p-3">
        <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-semibold text-primary">{index + 1}</span>
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex flex-col items-start gap-2 sm:flex-row sm:flex-wrap">
            <span className="min-w-0 flex-1 break-words text-sm font-medium">{objectName(row)}</span>
            <Badge variant="outline" className="shrink-0 text-xs">{t(`inventory.templates.resourceTypes.${row.resource_type}`, { defaultValue: definition?.label ?? row.resource_type })}</Badge>
          </div>
          <p className="break-words text-xs text-muted-foreground">{row.title}</p>
          <dl className="grid gap-1 text-xs">
            {known.map(({ field, value }) => <div key={field.key} className="grid gap-1 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)] sm:gap-2">
              <dt className="break-words text-muted-foreground">{templatePayloadFieldLabel(row.resource_type, field, t)}</dt>
              <dd className="break-words text-foreground" title={(field.references || field.referencesByName) && typeof value === "string" ? value : undefined}>{displayValue(value, field)}</dd>
            </div>)}
          </dl>
          <details className="text-xs text-muted-foreground">
            <summary className="cursor-pointer">{t("inventory.templates.run.technicalDetails")}</summary>
            <code className="break-all">{row.step_key}</code>
            {unknown.map(([key, value]) => <div key={key} className="break-all"><code>{key}</code>: {formatTemplateValue(value)}</div>)}
          </details>
        </div>
      </li>;
    })}
  </ol>;
}
