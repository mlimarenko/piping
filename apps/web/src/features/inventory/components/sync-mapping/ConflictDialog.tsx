import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";

/**
 * Version conflict dialog — shown when a 409 is returned on save.
 *
 * Options:
 *   [View changes in v{n+1}]         → navigate to new version in readonly
 *   [Discard and open v{n+1}]        → navigate to new version in edit
 *   [Continue editing (creates v{n+2})] → close dialog, user saves again
 *
 * Esc maps to the non-destructive default (View changes).
 *
 * Follow-up: show diff of changes in new version before discarding
 */

interface ConflictDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  newVersion: number;
  newId?: string;
  savedBy?: string;
  savedAt?: string;
  onContinueEditing: () => void;
}

const ConflictDialog = ({
  open,
  onOpenChange,
  newVersion,
  newId,
  savedBy,
  savedAt,
  onContinueEditing,
}: ConflictDialogProps) => {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const viewNew = () => {
    if (newId) void navigate(`/inventory/sync/mappings/${newId}/readonly`);
    onOpenChange(false);
  };

  const discardAndEdit = () => {
    if (newId) void navigate(`/inventory/sync/mappings/${newId}/edit`);
    onOpenChange(false);
  };

  // Esc maps to viewNew (non-destructive default)
  const handleOpenChange = (v: boolean) => {
    if (!v) viewNew();
    else onOpenChange(v);
  };

  const savedAtFormatted = savedAt
    ? new Date(savedAt).toLocaleString()
    : undefined;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="max-w-md surface border-border"
        aria-describedby="conflict-description"
      >
        <DialogHeader>
          <DialogTitle className="text-base">
            {t("sync.mapping.conflictTitle", "Version conflict")}
          </DialogTitle>
        </DialogHeader>

        <p id="conflict-description" className="text-sm text-muted-foreground">
          {savedBy && savedAtFormatted
            ? t(
                "sync.mapping.conflictDescriptionFull",
                "A newer version (v{{version}}) was saved by {{user}} at {{time}}.",
                { version: newVersion, user: savedBy, time: savedAtFormatted },
              )
            : t(
                "sync.mapping.conflictDescription",
                "A newer version (v{{version}}) was saved while you were editing.",
                { version: newVersion },
              )}
        </p>

        <DialogFooter className="flex-col sm:flex-row gap-2">
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-sm flex-1"
            onClick={viewNew}
          >
            {t(
              "sync.mapping.conflictViewChanges",
              "View changes in v{{version}}",
              { version: newVersion },
            )}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            className="h-9 text-sm flex-1"
            onClick={discardAndEdit}
          >
            {t(
              "sync.mapping.conflictDiscard",
              "Discard my edits and open v{{version}}",
              { version: newVersion },
            )}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm flex-1"
            onClick={onContinueEditing}
          >
            {t(
              "sync.mapping.conflictContinue",
              "Continue editing (creates v{{version}})",
              { version: newVersion + 1 },
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default ConflictDialog;
