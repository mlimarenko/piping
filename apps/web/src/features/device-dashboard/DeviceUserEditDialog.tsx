import { useRef, useState } from "react";
import { useIsMutating, useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";

import { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import {
  updateDeviceUsers,
  type DeviceUserUpdate,
  type DeviceUsers,
  type DeviceUsersOutcome,
} from "@/features/device-dashboard/api";

type DeviceUser = DeviceUsers["users"][number];

interface DeviceUserEditDialogProps {
  targetId: string;
  user: DeviceUser | null;
  authorityOptions: string[];
  onClose: () => void;
  onReturnFocus?: () => void;
  onApplied: () => void;
}

const DeviceUserEditDialog = ({
  targetId,
  user,
  authorityOptions,
  onClose,
  onReturnFocus,
  onApplied,
}: DeviceUserEditDialogProps) => {
  const { t } = useTranslation();
  const [password, setPassword] = useState("");
  const [group, setGroup] = useState(user?.group ?? "");
  const [authorities, setAuthorities] = useState(user?.authorities ?? []);
  const [formError, setFormError] = useState("");
  const submitting = useRef(false);
  const pendingWrites = useIsMutating({ mutationKey: ["device-user-write", targetId] });

  const mutation = useMutation({
    mutationKey: ["device-user-write", targetId],
    mutationFn: (updates: DeviceUserUpdate[]) => updateDeviceUsers(targetId, updates),
    onSuccess: () => onApplied(),
    onError: (error: unknown) =>
      notify("error", error instanceof ApiError ? error.message : t("unknownError", "Unknown error")),
    onSettled: () => { submitting.current = false; },
  });

  const submit = () => {
    if (!user || pendingWrites > 0 || submitting.current) return;
    const groupChanged = group.trim() !== (user.group ?? "");
    const authoritiesChanged = JSON.stringify(authorities) !== JSON.stringify(user.authorities);
    if (!password && !groupChanged && !authoritiesChanged) {
      setFormError(t("deviceDashboard.updateRequired", "Change at least one field."));
      return;
    }
    submitting.current = true;
    mutation.mutate([
      {
        name: user.name,
        ...(password ? { password } : {}),
        ...(groupChanged ? { group: group.trim() } : {}),
        ...(authoritiesChanged ? { authorities } : {}),
      },
    ]);
  };

  const outcome: DeviceUsersOutcome | undefined = mutation.data;
  return (
    <Dialog open={user !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md" onCloseAutoFocus={onReturnFocus ? (event) => { event.preventDefault(); onReturnFocus(); } : undefined}>
        <DialogHeader>
          <DialogTitle>
            {t("deviceDashboard.editUserTitle", "Edit {{name}}", { name: user?.name ?? "" })}
          </DialogTitle>
          <DialogDescription>
            {t(
              "deviceDashboard.editUserDescription",
              "Only changed fields are sent to the device. Leave password empty to keep it unchanged.",
            )}
          </DialogDescription>
        </DialogHeader>

        {outcome ? (
            <div aria-live="polite" className="space-y-2 text-sm">
            {outcome.succeeded.map((name) => (
              <p key={name} className="text-status-positive">
                {name}: {t("deviceDashboard.outcomeSucceeded", "Succeeded")}
              </p>
            ))}
            {outcome.failed.map((failure) => (
              <p key={failure.name} className="text-status-negative">
                {failure.name}: {failure.message}
              </p>
            ))}
          </div>
        ) : (
          <form
            id="device-user-edit-form"
            className="space-y-3"
            onSubmit={(event) => {
              event.preventDefault();
              submit();
            }}
          >
            <fieldset disabled={pendingWrites > 0} inert={pendingWrites > 0 ? true : undefined} className="space-y-3">
            <div className="space-y-1">
              <FieldLabel htmlFor="device-user-edit-password">
                {t("deviceDashboard.userPassword", "Password")}
              </FieldLabel>
              <Input
                id="device-user-edit-password"
                type="password"
                autoComplete="new-password"
                value={password}
                onChange={(event) => {
                  if (submitting.current || pendingWrites > 0) return;
                  setPassword(event.target.value);
                  setFormError("");
                }}
              />
            </div>
            <div className="space-y-1">
              <FieldLabel htmlFor="device-user-edit-group">
                {t("deviceDashboard.userGroup", "Group")}
              </FieldLabel>
              <Input
                id="device-user-edit-group"
                value={group}
                onChange={(event) => {
                  if (submitting.current || pendingWrites > 0) return;
                  setGroup(event.target.value);
                  setFormError("");
                }}
              />
            </div>
            <div className="space-y-1">
              <FieldLabel>{t("deviceDashboard.userAuthorities", "Authorities")}</FieldLabel>
              <MultiSelect
                disabled={pendingWrites > 0}
                items={[...new Set([...authorityOptions, ...authorities])].map((authority) => ({
                  id: authority,
                  label: authority,
                }))}
                value={authorities}
                onValueChange={(value) => {
                  if (submitting.current || pendingWrites > 0) return;
                  setAuthorities(value);
                  setFormError("");
                }}
                aria-label={t("deviceDashboard.userAuthorities", "Authorities")}
                placeholder={t("deviceDashboard.selectAuthorities", "Select authorities")}
                searchPlaceholder={t("deviceDashboard.searchAuthorities", "Search authorities")}
              />
            </div>
            {formError && (
              <p role="alert" className="text-xs text-status-negative">
                {formError}
              </p>
            )}
            </fieldset>
          </form>
        )}

        <DialogFooter>
          {outcome ? (
            <Button size="sm" onClick={onClose}>
              {t("close", "Close")}
            </Button>
          ) : (
            <>
              <Button variant="outline" size="sm" onClick={onClose}>
                {t("cancel", "Cancel")}
              </Button>
              <Button
                type="submit"
                form="device-user-edit-form"
                size="sm"
                disabled={pendingWrites > 0}
              >
                {mutation.isPending && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />}
                {t("save", "Save")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default DeviceUserEditDialog;
