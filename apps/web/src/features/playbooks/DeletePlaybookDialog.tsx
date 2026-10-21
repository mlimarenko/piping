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
import type { PlaybookCatalogItem } from "@/features/playbooks/api";

export function DeletePlaybookDialog({
  item,
  pending,
  onOpenChange,
  onConfirm,
}: Readonly<{
  item: PlaybookCatalogItem | null;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (item: PlaybookCatalogItem) => void;
}>) {
  const { t } = useTranslation();
  return (
    <Dialog open={item !== null} onOpenChange={(open) => { if (!pending) onOpenChange(open); }}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("playbooks.page.catalog.deleteTitle")}</DialogTitle>
          <DialogDescription>
            {t("playbooks.page.catalog.deleteDescription", {
              name: item?.playbook.name ?? "",
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
              if (item) {
                onConfirm(item);
              }
            }}
          >
            {t("delete")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
