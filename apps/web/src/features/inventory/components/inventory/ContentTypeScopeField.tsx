import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import { listContentTypes, type ContentType } from "@/features/inventory/custom-fields-api";

interface ContentTypeScopeFieldProps {
  contentTypes: readonly ContentType[];
  value: string[];
  onValueChange: (value: string[]) => void;
  label: string;
  valueKind: "id" | "token";
  capability: "custom-fields" | "tags";
  loading?: boolean;
  error?: boolean;
  disabled?: boolean;
}

/** One registry-backed scope picker. CFs bind install-local IDs; native Tags
 * bind portable tokens. Missing selections stay visible and are never dropped. */
export function ContentTypeScopeField({ contentTypes, value, onValueChange, label, valueKind, capability, loading, error, disabled }: Readonly<ContentTypeScopeFieldProps>) {
  const { t } = useTranslation();
  const items = useMemo(() => {
    const eligible = contentTypes.filter(type => capability === "tags" ? type.capabilities?.tags === true : type.supported_for_custom_fields);
    const options = eligible.map(type => ({ id: valueKind === "token" ? type.token : type.id, label: `${type.label} · ${type.token}`, disabled: false }));
    const known = new Set(options.map(option => option.id));
    for (const selected of value) {
      if (!known.has(selected)) options.push({ id: selected, label: `${t("customFields.def.unknownContentType")} · ${selected}`, disabled: true });
    }
    return options;
  }, [contentTypes, value, valueKind, capability, t]);
  return <div className="flex min-w-0 flex-col gap-2">
    <MultiSelect items={items} value={value} onValueChange={onValueChange} aria-label={label}
      placeholder={loading ? t("customFields.def.contentTypesLoading") : t(capability === "custom-fields" ? "customFields.def.contentTypePlaceholder" : "customFields.def.contentTypeSearchPlaceholder")}
      searchPlaceholder={t("customFields.def.contentTypeSearchPlaceholder")} emptyText={t("customFields.def.contentTypeEmpty")}
      disabled={disabled || loading || error} />
    {error && <p role="status" className="text-xs text-status-warning">{t("customFields.def.contentTypesError")}</p>}
  </div>;
}

export function TagObjectTypesField({ value, onValueChange, label, disabled }: Readonly<Pick<ContentTypeScopeFieldProps, "value" | "onValueChange" | "label" | "disabled">>) {
  const { t } = useTranslation();
  const catalog = useQuery({ queryKey: ["inventory-content-types"], queryFn: listContentTypes });
  return <div className="flex min-w-0 flex-col gap-2">
    <ContentTypeScopeField contentTypes={catalog.data ?? []} value={value} onValueChange={onValueChange}
      label={label} valueKind="token" capability="tags" loading={catalog.isPending} error={catalog.isError} disabled={disabled} />
    <p className="text-xs text-muted-foreground">{t("inventory.resources.fields.tagObjectTypesHint")}</p>
  </div>;
}
