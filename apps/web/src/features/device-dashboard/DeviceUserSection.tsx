import { useCallback, useRef, useState } from "react";
import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Pencil, Search, Trash2, UserPlus } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import { Input } from "@/shared/ui-primitives/input";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { getDeviceAuthorities, type DeviceNewUser, type DeviceUsers } from "@/features/device-dashboard/api";
import DeviceUserEditDialog from "@/features/device-dashboard/DeviceUserEditDialog";
import DeviceUserWriteDialog, {
  WritesDisabledBanner,
} from "@/features/device-dashboard/DeviceUserWriteDialog";
import type { PendingDeviceUserWrite } from "@/features/device-dashboard/deviceUserWrites";

type DeviceUser = DeviceUsers["users"][number];

interface DeviceUserSectionProps {
  targetId: string;
  users: DeviceUser[];
  /** `device_users:connection_target` and driver support permit adding users. */
  canCreate: boolean;
  /** `device_users:connection_target` and driver support permit editing users. */
  canUpdate: boolean;
  /** `device_users:connection_target` and driver support permit removing users. */
  canDelete: boolean;
  /** Driver structurally exposes its authority-token catalog. */
  hasAuthorityCatalog: boolean;
}

/**
 * The dashboard's Users section: the read-only account list plus, gated per
 * verdict, a per-account Remove control (`canDelete`) and an Add-user form
 * (`canCreate`). Every write is routed through {@link DeviceUserWriteDialog},
 * which previews the change (dry run) before an explicit Confirm applies it.
 */
const DeviceUserSection = ({
  targetId,
  users,
  canCreate,
  canUpdate,
  canDelete,
  hasAuthorityCatalog,
}: DeviceUserSectionProps) => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const [pending, setPending] = useState<PendingDeviceUserWrite | null>(null);
  const [editingUser, setEditingUser] = useState<DeviceUser | null>(null);
  const rowInvoker = useRef<HTMLElement | null>(null);
  const [isAddOpen, setIsAddOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [writesDisabled, setWritesDisabled] = useState(false);
  const pendingWrites = useIsMutating({ mutationKey: ["device-user-write", targetId] });

  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [group, setGroup] = useState("");
  const [authorities, setAuthorities] = useState<string[]>([]);
  const [customAuthority, setCustomAuthority] = useState("");
  const [formError, setFormError] = useState("");

  const authorityQuery = useQuery({
    queryKey: ["device", targetId, "authorities"],
    queryFn: () => getDeviceAuthorities(targetId),
    enabled: isAddOpen && hasAuthorityCatalog,
    retry: false,
  });
  const knownAuthorities = [...new Set([
    ...(authorityQuery.data?.authorities ?? []),
    ...users.flatMap((user) => user.authorities),
    ...authorities,
  ])].sort((left, right) => left.localeCompare(right));
  const authorityItems = knownAuthorities.map((authority) => ({ id: authority, label: authority }));
  const normalizedSearch = search.trim().toLocaleLowerCase();
  const visibleUsers = normalizedSearch
    ? users.filter((user) =>
        [user.name, user.group ?? "", ...user.authorities]
          .join(" ")
          .toLocaleLowerCase()
          .includes(normalizedSearch),
      )
    : users;

  const pendingRef = useRef<PendingDeviceUserWrite | null>(null);
  const nonceRef = useRef(0);

  const clearForm = useCallback(() => {
    setName("");
    setPassword("");
    setGroup("");
    setAuthorities([]);
    setCustomAuthority("");
    setFormError("");
  }, [setName, setPassword, setGroup, setAuthorities, setCustomAuthority, setFormError]);

  const refetchUsers = useCallback(() => {
    // Fire-and-forget: refetch errors surface through the query's own error
    // state, so a rejected invalidation here has nothing extra to report.
    queryClient
      .invalidateQueries({ queryKey: ["device", targetId, "users"] })
      .catch(() => undefined);
  }, [queryClient, targetId]);

  const onApplied = useCallback(() => {
    if (pendingRef.current?.kind === "create") {
      clearForm();
      setIsAddOpen(false);
    }
  }, [clearForm, setIsAddOpen]);

  const onWritesDisabled = useCallback(() => setWritesDisabled(true), []);

  const openPreview = (next: PendingDeviceUserWrite) => {
    pendingRef.current = next;
    setPending(next);
  };
  // A fresh id per action so repeating an identical write re-previews.
  const nextNonce = () => (nonceRef.current += 1);

  const startAdd = () => {
    const trimmedName = name.trim();
    if (!trimmedName) {
      setFormError(t("deviceDashboard.nameRequired", "A login name is required."));
      return;
    }
    setFormError("");
    const user: DeviceNewUser = {
      name: trimmedName,
      ...(password ? { password } : {}),
      ...(group.trim() ? { group: group.trim() } : {}),
      ...(authorities.length > 0 ? { authorities } : {}),
    };
    openPreview({
      kind: "create",
      title: t("deviceDashboard.addTitle", "Add device user"),
      nonce: nextNonce(),
      users: [user],
    });
  };

  const startRemove = (login: string) => {
    openPreview({
      kind: "delete",
      title: t("deviceDashboard.removeTitle", "Remove {{name}}", { name: login }),
      nonce: nextNonce(),
      usernames: [login],
    });
  };

  return (
    <div className="space-y-4">
      {(canCreate || canUpdate || canDelete) && writesDisabled && <WritesDisabledBanner />}

      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="relative min-w-0 flex-1 sm:max-w-sm">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("deviceDashboard.searchUsers", "Search users, groups, or authorities")}
            aria-label={t("deviceDashboard.searchUsers", "Search users, groups, or authorities")}
            className="h-9 pl-9"
          />
        </div>
        <div className="flex items-center gap-2">
          <Badge variant="status-neutral">
            {t("deviceDashboard.userCount", "{{count}} users", { count: users.length })}
          </Badge>
          {canCreate && (
            <Button
              type="button"
              size="sm"
              className="gap-1.5"
              disabled={writesDisabled || pendingWrites > 0}
              onClick={() => setIsAddOpen(true)}
            >
              <UserPlus className="h-4 w-4" />
              {t("deviceDashboard.addUser", "Add user")}
            </Button>
          )}
        </div>
      </div>

      {visibleUsers.length === 0 ? (
        <p className="py-10 text-center text-sm text-muted-foreground">
          {users.length === 0
            ? t("deviceDashboard.noUsers", "No user accounts reported.")
            : t("deviceDashboard.noUsersFound", "No users match your search.")}
        </p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="border-b border-border text-xs uppercase tracking-wide text-muted-foreground">
                <th className="py-1.5 pr-3 font-medium">{t("deviceDashboard.userName", "Name")}</th>
                <th className="py-1.5 pr-3 font-medium">{t("deviceDashboard.userGroup", "Group")}</th>
                <th className="py-1.5 font-medium">
                  {t("deviceDashboard.userAuthorities", "Authorities")}
                </th>
                {(canUpdate || canDelete) && (
                  <th className="w-16 py-1.5">
                    <span className="sr-only">{t("actions")}</span>
                  </th>
                )}
              </tr>
            </thead>
            <tbody>
              {visibleUsers.map((user) => (
                <UserRow
                  key={user.name}
                  user={user}
                  canUpdate={canUpdate}
                  canDelete={canDelete}
                  writesDisabled={writesDisabled || pendingWrites > 0}
                  onEdit={(invoker) => { rowInvoker.current = invoker; setEditingUser(user); }}
                  onRemove={(invoker) => { rowInvoker.current = invoker; startRemove(user.name); }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      {canCreate && (
        <Dialog open={isAddOpen} onOpenChange={setIsAddOpen}>
          <DialogContent className="sm:max-w-xl">
            <DialogHeader>
              <DialogTitle>{t("deviceDashboard.addUser", "Add user")}</DialogTitle>
              <DialogDescription>
                {t(
                  "deviceDashboard.addUserDescription",
                  "Choose the account details and device authorities, then preview the change before applying it.",
                )}
              </DialogDescription>
            </DialogHeader>
            <AddUserForm
              name={name}
              password={password}
              group={group}
              authorities={authorities}
              authorityItems={authorityItems}
              customAuthority={customAuthority}
              error={formError}
               disabled={writesDisabled || pendingWrites > 0}
              onNameChange={(value) => {
                setName(value);
                setFormError("");
              }}
              onPasswordChange={setPassword}
              onGroupChange={setGroup}
              onAuthoritiesChange={setAuthorities}
              onCustomAuthorityChange={setCustomAuthority}
              onAddCustomAuthority={() => {
                const token = customAuthority.trim();
                if (!token || authorities.includes(token)) return;
                setAuthorities([...authorities, token]);
                setCustomAuthority("");
              }}
              onSubmit={startAdd}
            />
          </DialogContent>
        </Dialog>
      )}

      {editingUser && (
        <DeviceUserEditDialog
          key={`${targetId}:${editingUser.name}`}
          targetId={targetId}
          user={editingUser}
          authorityOptions={knownAuthorities}
          onClose={() => setEditingUser(null)}
          onReturnFocus={() => { rowInvoker.current?.focus(); rowInvoker.current = null; }}
          onApplied={refetchUsers}
        />
      )}
      <DeviceUserWriteDialog
        pending={pending}
        targetId={targetId}
        onClose={() => { pendingRef.current = null; setPending(null); }}
        onApplied={onApplied}
        onWritesDisabled={onWritesDisabled}
        onReturnFocus={() => { const invoker = rowInvoker.current; rowInvoker.current = null; if (!invoker?.isConnected) return false; invoker.focus(); return true; }}
      />
    </div>
  );
};

const UserRow = ({
  user,
  canUpdate,
  canDelete,
  writesDisabled,
  onEdit,
  onRemove,
}: {
  user: DeviceUser;
  canUpdate: boolean;
  canDelete: boolean;
  writesDisabled: boolean;
  onEdit: (invoker: HTMLButtonElement | null) => void;
  onRemove: (invoker: HTMLButtonElement | null) => void;
}) => {
  const { t } = useTranslation();
  const triggerRef = useRef<HTMLButtonElement>(null);
  return (
    <tr className="group border-b border-border align-top last:border-0">
      <td className="py-2 pr-3 font-medium text-foreground">{user.name}</td>
      <td className="py-2 pr-3 text-muted-foreground">{user.group?.trim() || "—"}</td>
      <td className="max-w-[32rem] py-2">
        <div className="flex flex-wrap items-center gap-1">
          {user.authorities.slice(0, 3).map((authority) => (
            <Badge key={authority} variant="secondary" className="max-w-40 truncate text-xs font-normal">
              {authority}
            </Badge>
          ))}
          {user.authorities.length > 3 && (
            <Badge
              variant="status-neutral"
              title={user.authorities.join(", ")}
              className="shrink-0 text-xs"
            >
              +{user.authorities.length - 3}
            </Badge>
          )}
          {user.authorities.length === 0 && <span className="text-muted-foreground">—</span>}
        </div>
      </td>
      {(canUpdate || canDelete) && (
        <td className="py-2 text-right">
          <RowActions ref={triggerRef} label={`${t("actions")}: ${user.name}`} disabled={writesDisabled}>
            {canUpdate && (
              <DropdownMenuItem
                onSelect={() => onEdit(triggerRef.current)}
              >
                <Pencil className="h-3.5 w-3.5" />
                {t("deviceDashboard.editUser", "Edit {{name}}", { name: user.name })}
              </DropdownMenuItem>
            )}
            {canDelete && (
              <DropdownMenuItem variant="destructive"
                onSelect={() => onRemove(triggerRef.current)}
              >
                <Trash2 className="h-3.5 w-3.5" />
                {t("deviceDashboard.removeUser", "Remove {{name}}", { name: user.name })}
              </DropdownMenuItem>
            )}
          </RowActions>
        </td>
      )}
    </tr>
  );
};

interface AddUserFormProps {
  name: string;
  password: string;
  group: string;
  authorities: string[];
  authorityItems: Array<{ id: string; label: string }>;
  customAuthority: string;
  error: string;
  disabled: boolean;
  onNameChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onGroupChange: (value: string) => void;
  onAuthoritiesChange: (value: string[]) => void;
  onCustomAuthorityChange: (value: string) => void;
  onAddCustomAuthority: () => void;
  onSubmit: () => void;
}

const AddUserForm = ({
  name,
  password,
  group,
  authorities,
  authorityItems,
  customAuthority,
  error,
  disabled,
  onNameChange,
  onPasswordChange,
  onGroupChange,
  onAuthoritiesChange,
  onCustomAuthorityChange,
  onAddCustomAuthority,
  onSubmit,
}: AddUserFormProps) => {
  const { t } = useTranslation();
  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit();
      }}
    >
      <div className="flex items-center gap-2">
        <UserPlus className="h-3.5 w-3.5 text-muted-foreground" />
        <h3 className="text-sm font-semibold text-foreground">
          {t("deviceDashboard.addUser", "Add user")}
        </h3>
      </div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="space-y-1">
          <FieldLabel required htmlFor="device-user-name">
            {t("deviceDashboard.userName", "Name")}
          </FieldLabel>
          <Input
            id="device-user-name"
            className="h-8 text-sm"
            value={name}
            disabled={disabled}
            aria-invalid={error ? true : undefined}
            onChange={(event) => onNameChange(event.target.value)}
          />
        </div>
        <div className="space-y-1">
          <FieldLabel htmlFor="device-user-password">
            {t("deviceDashboard.userPassword", "Password")}
          </FieldLabel>
          <Input
            id="device-user-password"
            type="password"
            autoComplete="new-password"
            className="h-8 text-sm"
            value={password}
            disabled={disabled}
            onChange={(event) => onPasswordChange(event.target.value)}
          />
        </div>
        <div className="space-y-1">
          <FieldLabel htmlFor="device-user-group">
            {t("deviceDashboard.userGroup", "Group")}
          </FieldLabel>
          <Input
            id="device-user-group"
            className="h-8 text-sm"
            value={group}
            disabled={disabled}
            onChange={(event) => onGroupChange(event.target.value)}
          />
        </div>
        <div className="space-y-1 sm:col-span-2">
          <FieldLabel>
            {t("deviceDashboard.userAuthorities", "Authorities")}
          </FieldLabel>
          <MultiSelect
            items={authorityItems}
            value={authorities}
            onValueChange={onAuthoritiesChange}
            disabled={disabled}
            aria-label={t("deviceDashboard.userAuthorities", "Authorities")}
            placeholder={t("deviceDashboard.selectAuthorities", "Select authorities")}
            searchPlaceholder={t("deviceDashboard.searchAuthorities", "Search authorities")}
            emptyText={t("deviceDashboard.noAuthorities", "No authority tokens found")}
          />
          <div className="flex gap-2">
            <Input
              value={customAuthority}
              disabled={disabled}
              className="h-8 text-sm"
              placeholder={t("deviceDashboard.customAuthority", "Add an exact device token")}
              aria-label={t("deviceDashboard.customAuthority", "Add an exact device token")}
              onChange={(event) => onCustomAuthorityChange(event.target.value)}
            />
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={disabled || !customAuthority.trim()}
              onClick={onAddCustomAuthority}
            >
              {t("add", "Add")}
            </Button>
          </div>
        </div>
      </div>
      {error ? (
        <p role="alert" className="text-xs text-destructive">{error}</p>
      ) : (
        <p className="text-xs text-muted-foreground">
          {t(
            "deviceDashboard.authoritiesHint",
            "Choose tokens reported by the device. Add a custom token only when you know its exact value.",
          )}
        </p>
      )}
      <div className="flex justify-end">
        <Button type="submit" size="sm" disabled={disabled}>
          {t("deviceDashboard.previewChanges", "Preview changes")}
        </Button>
      </div>
    </form>
  );
};

export default DeviceUserSection;
