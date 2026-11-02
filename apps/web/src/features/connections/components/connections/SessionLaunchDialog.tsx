import { useTranslation } from "react-i18next";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import { AdHocCredentialsForm } from "./AdHocCredentialsDialog";
import type { useSessionLaunch } from "../../hooks/useSessionLaunch";

export default function SessionLaunchDialog<D>({ controller }: { controller: ReturnType<typeof useSessionLaunch<D>> }) {
  const { t } = useTranslation();
  const intent = controller.intent;
  if (!intent) return null;
  if (!intent.replacementId && !intent.context) return null;
  const cancel = () => controller.cancel(intent.attempt);
  const restoreFocus = (event: Event) => controller.restoreFocus(event, intent.attempt);
  return <Dialog open onOpenChange={open => { if (!open) cancel(); }}>
    {intent.context ? <AdHocCredentialsForm key={`${intent.attempt}:${intent.context.account.entryId}`} context={intent.context} onOpenChange={open => { if (!open) cancel(); }} onSubmit={input => controller.submit(input, intent.attempt)} onCloseAutoFocus={restoreFocus} /> : <DialogContent onCloseAutoFocus={restoreFocus}>
      <DialogHeader><DialogTitle>{t("connections.reconnectSession")} · {intent.target.name}</DialogTitle></DialogHeader>
      <DialogDescription>{t("connections.retryLaunch.chooseAccount")}</DialogDescription>
      {intent.error ? <p role="alert">{intent.error}</p> : null}
      {!intent.accounts && !intent.error ? <p role="status">{t("loading")}</p> : null}
      <div className="flex flex-col gap-2">
        {intent.accounts?.map(account => <Button key={account.entryId} disabled={intent.submitting} variant="outline" onClick={() => void controller.choose(account, intent.attempt)}>{controller.accountText(account, intent.protocol)}</Button>)}
        {intent.error && !intent.accounts?.length ? <Button onClick={() => {
          void controller.refresh(intent.attempt);
        }}>{t("retry")}</Button> : null}
        <Button variant="ghost" onClick={cancel}>{t("cancel")}</Button>
      </div>
    </DialogContent>}
  </Dialog>;
}
