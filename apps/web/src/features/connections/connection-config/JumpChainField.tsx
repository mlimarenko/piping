import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import type { ConnConfJumpHop, ConnConfRecord } from "./api";

// Only inspect the jump leaf. Missing bases, cycles and malformed values remain
// server decisions; they are not evidence that a route is nested.
function hasNestedJump(record: ConnConfRecord, byId: Map<string, ConnConfRecord>): boolean {
  const seen = new Set<string>();
  let current: ConnConfRecord | undefined = record;
  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    const params = current.params;
    if (!params || typeof params !== "object" || Array.isArray(params)) return false;
    if ("jump" in params) {
      const jump = params.jump;
      return Array.isArray(jump) && jump.length > 0;
    }
    current = current.base_record_id ? byId.get(current.base_record_id) : undefined;
  }
  return false;
}

export default function JumpChainField({ value, records, recordId, onChange }: {
  value: ConnConfJumpHop[] | undefined;
  records: ConnConfRecord[];
  recordId?: string;
  onChange: (value: ConnConfJumpHop[]) => void;
}) {
  const { t } = useTranslation();
  const id = useId();
  // Params originate from the API's free-form JSON; do not silently replace malformed data.
  const valid = value === undefined || (Array.isArray(value) && value.every(
    hop => hop && typeof hop === "object" && typeof hop.connconf_record_id === "string" && hop.connconf_record_id.length > 0,
  ));
  const hops = valid && Array.isArray(value) ? value : [];
  const selected = new Set(hops.map(hop => hop.connconf_record_id));
  const eligible = records.filter(record => record.protocol === "ssh" && record.enabled && record.id !== recordId);
  const byId = new Map(records.map(record => [record.id, record]));
  const nested = new Set(eligible.filter(record => hasNestedJump(record, byId)).map(record => record.id));
  const options = eligible.filter(record => !selected.has(record.id));
  const nestedLabel = (name: string) => t("connectionConfig.fields.jumpNestedUnsupported", { defaultValue: "{{name}}: nested jump chains are unsupported", name });
  const move = (index: number, direction: number) => {
    const next = [...hops];
    const target = index + direction;
    const sourceHop = next[index];
    const targetHop = next[target];
    if (!sourceHop || !targetHop) return;
    [next[index], next[target]] = [targetHop, sourceHop];
    onChange(next);
  };

  return <div className="flex flex-col gap-2 border-t border-border pt-4">
    <FieldLabel htmlFor={id}>{t("connectionConfig.fields.jumpChain", { defaultValue: "SSH jump chain" })}</FieldLabel>
    <p className="text-xs text-muted-foreground">{t("connectionConfig.fields.jumpHint", { defaultValue: "Hops are used in order and require private-key authentication. Nested jump chains are unsupported. The server validates cycles, authentication, bindings and host key pins." })}</p>
    {!valid ? <p role="alert" className="text-sm text-destructive">{t("connectionConfig.fields.jumpInvalid", { defaultValue: "The stored jump chain is invalid and cannot be edited here." })}</p> : <>
      <ol className="flex flex-col gap-2">
        {hops.map((hop, index) => {
          const record = eligible.find(item => item.id === hop.connconf_record_id);
          const label = record ? (nested.has(record.id) ? nestedLabel(record.name) : record.name) : t("connectionConfig.fields.jumpUnavailable", { defaultValue: "Unavailable: {{id}}", id: hop.connconf_record_id });
          return <li key={`${hop.connconf_record_id}-${index}`} className="flex flex-wrap items-center gap-2 text-sm">
            <span className="flex-1">{index + 1}. {label}</span>
            <Button type="button" variant="outline" size="sm" disabled={index === 0} aria-label={t("connectionConfig.fields.jumpMoveUp", { defaultValue: "Move {{name}} up", name: label })} onClick={() => move(index, -1)}>↑</Button>
            <Button type="button" variant="outline" size="sm" disabled={index === hops.length - 1} aria-label={t("connectionConfig.fields.jumpMoveDown", { defaultValue: "Move {{name}} down", name: label })} onClick={() => move(index, 1)}>↓</Button>
            <Button type="button" variant="outline" size="sm" aria-label={t("connectionConfig.fields.jumpRemove", { defaultValue: "Remove {{name}}", name: label })} onClick={() => onChange(hops.filter((_, i) => i !== index))}>{t("connectionConfig.fields.jumpRemoveButton", { defaultValue: "Remove" })}</Button>
          </li>;
        })}
      </ol>
      <EntitySelect id={id} aria-label={t("connectionConfig.fields.jumpAdd", { defaultValue: "Add SSH jump hop" })} value="" items={options.map(record => ({ id: record.id, label: nested.has(record.id) ? nestedLabel(record.name) : record.name, disabled: nested.has(record.id) }))}
        placeholder={t("connectionConfig.fields.jumpAdd", { defaultValue: "Add SSH jump hop" })}
        onValueChange={recordId => { if (!nested.has(recordId) && options.some(record => record.id === recordId)) onChange([...hops, { connconf_record_id: recordId }]); }} />
    </>}
  </div>;
}
