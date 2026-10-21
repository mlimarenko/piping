import { useTranslation } from "react-i18next";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";

export function DeleteTargetSetDialog({
  open,
  name,
  pending,
  onOpenChange,
  onConfirm,
}: Readonly<{
  open: boolean;
  name: string;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}>) {
  const { t } = useTranslation();
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>{t("playbooks.page.targetSets.deleteTitle")}</AlertDialogTitle>
          <AlertDialogDescription>
            {t("playbooks.page.targetSets.deleteDescription", {
              name,
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t("playbooks.page.targetSets.deleteImpact")}
        </p>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>
            {t("cancel")}
          </AlertDialogCancel>
          <AlertDialogAction
            className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            disabled={pending}
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
          >
            {t("delete")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
