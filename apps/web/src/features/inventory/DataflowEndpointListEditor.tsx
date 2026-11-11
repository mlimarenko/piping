import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import DataflowEndpointPicker from "./DataflowEndpointPicker";
import { dataflowEndpointError } from "./dataflowEndpoints";
import type { DataflowEndpoint } from "./dataflows-api";

export const DATAFLOW_SIDE_LIMIT = 32;

function newEndpoint(): DataflowEndpoint {
  return { object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] };
}

function newRowKey(): string {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

/** One ordered side of a flow. The wire payload is the array itself; row keys
 * exist only to keep pickers stable while rows are added or removed. */
export default function DataflowEndpointListEditor({ label, rowLabel = label, addLabel, value, onChange, disabled = false, showErrors = false }: Readonly<{
  label: string;
  rowLabel?: string;
  addLabel: string;
  value: DataflowEndpoint[];
  onChange: (value: DataflowEndpoint[]) => void;
  disabled?: boolean;
  showErrors?: boolean;
}>) {
  const { t } = useTranslation();
  const [rowKeys, setRowKeys] = useState(() => value.map(newRowKey));
  const valueLength = value.length;
  const fallbackKeys = useMemo(() => Array.from({ length: valueLength }, newRowKey), [valueLength]);
  const keys = rowKeys.length === valueLength ? rowKeys : fallbackKeys;
  const empty = value.length === 0;
  const atLimit = value.length >= DATAFLOW_SIDE_LIMIT;
  const change = (index: number, endpoint: DataflowEndpoint) => {
    onChange(value.map((current, row) => row === index ? endpoint : current));
  };
  const remove = (index: number) => {
    setRowKeys(current => current.filter((_, row) => row !== index));
    onChange(value.filter((_, row) => row !== index));
  };
  const add = () => {
    setRowKeys(current => [...current, newRowKey()]);
    onChange([...value, newEndpoint()]);
  };
  return <fieldset data-endpoint-invalid={empty || value.length > DATAFLOW_SIDE_LIMIT} tabIndex={-1}
    className="flex min-w-0 flex-col gap-3 outline-none">
    <legend className="text-sm font-medium">{label}</legend>
    <p className="text-xs text-muted-foreground">{t("dataflows.sideLimit")}</p>
    {empty && showErrors && <p role="alert" className="text-xs text-destructive">{t("dataflows.endpointRequired")}</p>}
    <ul className="flex min-w-0 flex-col gap-3">
      {value.map((endpoint, index) => {
        const error = dataflowEndpointError(endpoint);
        return <li key={keys[index] ?? index} data-endpoint-invalid={!!error} className="min-w-0">
          <div className="flex min-w-0 items-start gap-2">
            <div className="min-w-0 flex-1">
              <DataflowEndpointPicker label={`${rowLabel} ${index + 1}`} value={endpoint} onChange={next => change(index, next)} disabled={disabled} />
            </div>
            <Button type="button" variant="ghost" size="icon" className="mt-7 shrink-0 text-muted-foreground hover:text-destructive"
              aria-label={`${t("dataflows.removeEndpoint")} ${rowLabel} ${index + 1}`} disabled={disabled} onClick={() => remove(index)}>
              <Trash2 aria-hidden="true" className="size-4" />
            </Button>
          </div>
          {showErrors && error && <p role="alert" className="pt-1 text-xs text-destructive">{t(error)}</p>}
        </li>;
      })}
    </ul>
    {!atLimit && <Button type="button" variant="outline" size="sm" className="self-start" onClick={add} disabled={disabled}>
      <Plus aria-hidden="true" className="size-4" />{addLabel}
    </Button>}
  </fieldset>;
}
