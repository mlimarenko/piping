import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listAllGroups, listAllUsers, USERS_QUERY_KEY } from "@/features/access-control/api";
import { usePermissions } from "@/shared/auth/permissionsContext";
import MultiSelect from "@/shared/ui-shared/MultiSelect";

interface IdentitySelectionProps {
  kind: "user" | "group";
  value: string[];
  label: string;
  onChange?: (ids: string[]) => void;
  disabled?: boolean;
}

/** Directory UUID selection and display share the canonical paginated directory APIs. */
export default function IdentitySelection({ kind, value, label, onChange, disabled }: Readonly<IdentitySelectionProps>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const canRead = can("READ", kind === "user" ? "USER" : "GROUP");
  const query = useQuery({
    queryKey: kind === "user" ? [...USERS_QUERY_KEY, "identity-options"] : ["access", "groups", "identity-options"],
    queryFn: async () => kind === "user"
      ? (await listAllUsers()).map((user) => ({ id: String(user.id), label: user.username, searchText: user.email }))
      : (await listAllGroups()).map((group) => ({ id: String(group.id), label: group.name })),
    enabled: canRead,
    staleTime: 60_000,
  });
  const items = query.data ?? [];
  const byId = new Map<string, string>(items.map((item) => [item.id, item.label]));
  // Unresolvable assignments remain visible and intact; never silently drop an ID.
  const selectedItems = value.filter((id) => !byId.has(id)).map((id) => ({ id, label: id }));
  const selectedLabels: string[] = value.map((id) => byId.get(id) ?? id);
  if (!onChange) return <span className="break-words text-muted-foreground">{selectedLabels.join(", ") || "—"}</span>;
  return (
    <div className="flex flex-col gap-1">
      <MultiSelect items={[...items, ...selectedItems]} value={value} onValueChange={onChange}
        aria-label={label} disabled={disabled || !canRead || query.isPending || query.isError} />
      {(!canRead || query.isError) && <output className="text-xs text-muted-foreground">{t("inventory.resources.form.identityUnavailable")}</output>}
    </div>
  );
}
