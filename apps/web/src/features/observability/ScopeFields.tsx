import { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { searchConnectionTargets } from "@/features/connections/api";
import { Input } from "@/shared/ui-primitives/input";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import {
  MONITORING_SCOPE_I18N_KEY,
  MONITORING_SCOPE_LABEL,
  MONITORING_SCOPE_TYPES,
  type MonitoringScopeType,
} from "./scope";
import type { ScopeEntities } from "./useScopeEntities";

const CONNECTION_TARGET_SEARCH_LIMIT = 50;
const CONNECTION_TARGET_DEBOUNCE_MS = 250;

interface ScopeFieldsProps {
  scopeType: MonitoringScopeType;
  scopeId: string;
  onScopeTypeChange: (next: MonitoringScopeType) => void;
  onScopeIdChange: (next: string) => void;
  entities: ScopeEntities;
  disabled?: boolean;
}

function ConnectionTargetScopePicker({
  value,
  onChange,
  disabled = false,
  id,
  selectedLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  disabled?: boolean;
  id: string;
  selectedLabel: string;
}) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const canRead = can("READ", "CONNECTION_TARGET");
  const [open, setOpen] = useState(false);
  const [term, setTerm] = useState("");
  const [debounced, setDebounced] = useState("");
  const debounceTimer = useRef<number | undefined>(undefined);
  const [nameCache, setNameCache] = useState<Map<string, string>>(new Map());

  const optionsQuery = useQuery({
    queryKey: ["observability", "connection-target-scope-search", debounced],
    queryFn: () => searchConnectionTargets(debounced, CONNECTION_TARGET_SEARCH_LIMIT),
    enabled: open && canRead,
    staleTime: 30_000,
  });

  const options = canRead && !optionsQuery.isError ? optionsQuery.data ?? [] : [];
  const selectedName = value ? (canRead ? nameCache.get(value) : undefined) ?? selectedLabel : null;

  const handleTermChange = (next: string) => {
    setTerm(next);
    if (debounceTimer.current) window.clearTimeout(debounceTimer.current);
    debounceTimer.current = window.setTimeout(
      () => setDebounced(next),
      CONNECTION_TARGET_DEBOUNCE_MS,
    );
  };

  return (
    <SearchableSelect
      id={id}
      disabled={disabled}
      value={value}
      selectedLabel={selectedName}
      onValueChange={(next) => {
        if (disabled) return;
        const selected = options.find((option) => option.id === next);
        if (selected) {
          setNameCache((current) => new Map(current).set(selected.id, selected.name));
        }
        onChange(next);
        setTerm("");
        setDebounced("");
      }}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setTerm("");
          setDebounced("");
        }
      }}
      items={options.map((option) => ({
        id: option.id,
        label: option.name,
        searchText: option.id,
      }))}
      shouldFilter={false}
      searchValue={term}
      onSearchValueChange={handleTermChange}
      isLoading={open && canRead && optionsQuery.isPending}
      placeholder={t("observability.scope.pickTarget", {
        defaultValue: "Pick a target…",
      })}
      searchPlaceholder={t("observability.scope.searchConnectionTarget", {
        defaultValue: "Search connection targets…",
      })}
      emptyText={t(!canRead ? "observability.workflow.nameUnavailable" : optionsQuery.isError ? "observability.entityLoadFailed" : "noResults")}
    />
  );
}

/**
 * Scope-type select + dependent target picker. `global` carries no target;
 * every other layer requires a concrete entity id.
 */
const ScopeFields = ({
  scopeType,
  scopeId,
  onScopeTypeChange,
  onScopeIdChange,
  entities,
  disabled = false,
}: ScopeFieldsProps) => {
  const { t } = useTranslation();
  const targetId = useId();
  const targetOptions = scopeType === "global" ? [] : entities.optionsByScope[scopeType] ?? [];

  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <FormSelect
        disabled={disabled}
        value={scopeType}
        onValueChange={(value) => {
          if (disabled) return;
          onScopeTypeChange(value);
          onScopeIdChange("");
        }}
        label={t("observability.scope.typeLabel", { defaultValue: "Scope" })}
        options={MONITORING_SCOPE_TYPES.map((scope) => ({
          value: scope,
          label: t(`observability.scope.${MONITORING_SCOPE_I18N_KEY[scope]}`, {
            defaultValue: MONITORING_SCOPE_LABEL[scope],
          }),
        }))}
      />
      <FormField
        htmlFor={targetId}
        label={t("observability.scope.targetLabel", { defaultValue: "Target" })}
        required={scopeType !== "global"}
      >
        {scopeType === "global" ? (
          <Input
            id={targetId}
            value={t("observability.scope.appliesAll", { defaultValue: "Applies everywhere" })}
            disabled
          />
        ) : scopeType === "connection_target" ? (
          <ConnectionTargetScopePicker id={targetId} value={scopeId} selectedLabel={entities.resolveName("connection_target", scopeId)} onChange={onScopeIdChange} disabled={disabled} />
        ) : (
          <EntitySelect
            id={targetId}
            disabled={disabled}
            value={scopeId}
            onValueChange={(value) => { if (!disabled) onScopeIdChange(value); }}
            placeholder={t("observability.scope.pickTarget", {
              defaultValue: "Pick a target…",
            })}
            searchPlaceholder={t("search")}
            items={targetOptions.map((option) => ({ id: option.id, label: option.name }))}
          />
        )}
      </FormField>
    </div>
  );
};

export default ScopeFields;
