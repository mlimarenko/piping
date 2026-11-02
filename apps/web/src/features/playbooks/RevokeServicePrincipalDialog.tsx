import { useTranslation } from "react-i18next";

import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import type { PlaybookServicePrincipal } from "@/features/playbooks/api";

export function RevokeServicePrincipalDialog({
  principal,
  pending,
  onOpenChange,
  onConfirm,
}: Readonly<{
  principal: PlaybookServicePrincipal | null;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (principal: PlaybookServicePrincipal) => void;
}>) {
  const { t } = useTranslation();
  return (
    <Dialog open={principal !== null} onOpenChange={(open) => { if (!pending) onOpenChange(open); }}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("playbooks.page.servicePrincipals.revokeTitle")}</DialogTitle>
          <DialogDescription>
            {t("playbooks.page.servicePrincipals.revokeDescription", {
              name: principal?.name ?? "",
            })}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" disabled={pending} onClick={() => onOpenChange(false)}>
            {t("cancel")}
          </Button>
          <Button
            variant="destructive"
            disabled={pending}
            onClick={() => {
              if (principal) {
                onConfirm(principal);
              }
            }}
          >
            {t("playbooks.page.actions.revoke")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
