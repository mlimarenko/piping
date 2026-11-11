import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { fetchSecretFieldStates } from "@/shared/settings/api";
import SecretField from "@/shared/ui-shared/SecretField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Button } from "@/shared/ui-primitives/button";

interface ManagedSecretFieldProps {
  label: string;
  consumerType: string;
  consumerId?: string;
  fieldKey: string;
  intent: string;
  value: string;
  disabled?: boolean;
  canReveal: boolean;
  onIntent: (intent: string) => void;
  onValue: (value: string) => void;
  reveal: (id: string) => Promise<string>;
}

export default function ManagedSecretField({ label, consumerType, consumerId, fieldKey,
  intent, value, disabled, canReveal, onIntent, onValue, reveal }: ManagedSecretFieldProps) {
  const { t } = useTranslation();
  const [hasValue, setHasValue] = useState<boolean>();
  const [stateFailed, setStateFailed] = useState(false);
  const [revealed, setRevealed] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [denied, setDenied] = useState(false);
  useEffect(() => {
    let current = true;
    if (consumerId) {
      void fetchSecretFieldStates(consumerType, consumerId).then(states => {
        if (current) setHasValue(states.find(state => state.field_key === fieldKey)?.has_value ?? false);
      }).catch(() => { if (current) setStateFailed(true); });
    }
    return () => { current = false; };
  }, [consumerType, consumerId, fieldKey]);

  const revealValue = async () => {
    if (revealed !== undefined) { setRevealed(undefined); return; }
    if (!consumerId || !canReveal) return;
    setBusy(true);
    try { setRevealed(await reveal(consumerId)); }
    catch { setDenied(true); }
    finally { setBusy(false); }
  };

  return <div className="flex min-w-0 flex-col gap-2">
    {consumerId && <p className="text-xs text-muted-foreground" role="status">
      {t(stateFailed ? "managedSecret.stateFailed" : hasValue === undefined ? "loading" : hasValue ? "managedSecret.configured" : "managedSecret.missing")}
    </p>}
    <FormSelect aria-label={label} value={intent || "preserve"} disabled={disabled}
      onValueChange={next => { onIntent(next === "preserve" ? "" : next); onValue(""); setRevealed(undefined); }}
      options={["preserve", "set", "rotate", "clear"].map(action => ({ value: action, label: t(`managedSecret.${action}`) }))} />
    {(intent === "set" || intent === "rotate") && <SecretField disabled={disabled} label={label} value={value} onChange={next => onValue(next ?? "")} />}
    {intent === "clear" && <p className="text-xs text-destructive">{t("managedSecret.clearOnSave")}</p>}
    {canReveal && consumerId && hasValue && !denied && <Button type="button" variant="outline" size="sm" className="self-start" disabled={busy}
      onClick={() => void revealValue()}>{t(revealed === undefined ? "customFields.entity.reveal" : "customFields.entity.hide")}</Button>}
    {canReveal && revealed !== undefined && <output className="break-all font-mono text-sm">{revealed}</output>}
    {denied && <p role="alert" className="text-xs text-destructive">{t("managedSecret.revealDenied")}</p>}
  </div>;
}
