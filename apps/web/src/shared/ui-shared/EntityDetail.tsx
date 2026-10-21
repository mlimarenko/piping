import { useRef, useState, type ComponentProps, type ReactNode } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import { ArrowLeft, type LucideIcon } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import ConfirmDeleteDialog from "./ConfirmDeleteDialog";
import PageChrome from "@/shared/ui-shared/PageChrome";
import StatusChip from "@/shared/ui-shared/StatusChip";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import DefinitionGrid, { type DefinitionGridField } from "./DefinitionGrid";

export interface EntityDetailField {
  label: string;
  value: ReactNode;
}

export interface EntityDetailStatus {
  tone: ComponentProps<typeof StatusChip>["status"];
  label: string;
}

export interface EntityDetailAction {
  key: string;
  label: string;
  icon?: LucideIcon;
  /** Renders the action as a link to this route (e.g. the edit page). */
  href?: string;
  /** Runs immediately when the action is pressed (no confirmation dialog). */
  onClick?: () => void;
  variant?: "default" | "outline" | "ghost";
  /** Pre-gated by the domain; disallowed actions are not rendered. */
  allowed: boolean;
  disabled?: boolean;
  /** When set, pressing the action opens a confirmation dialog before `onConfirm`. */
  confirm?: {
    title: string;
    description: ReactNode;
    confirmLabel?: string;
    onConfirm: () => void;
  };
}

export interface EntityDetailProps {
  icon: LucideIcon;
  title: string;
  description?: string;
  backHref: string;
  backLabel?: string;
  status?: EntityDetailStatus;
  fields: EntityDetailField[];
  actions: EntityDetailAction[];
  /** Surfaced as an `role="alert"` line between the header and the field grid. */
  error?: ReactNode;
  children?: ReactNode;
}

/**
 * Pure read-only entity-detail frame: back link, header chrome with pre-gated
 * actions, a definition grid, an optional error line and an optional body.
 *
 * Owns rendering only — no `useQuery`, no `useApiMutation`, no `can()`. Data
 * fetch, capability gates, delete mutation, notifications and navigation stay in
 * the domain and are passed in as pre-computed props/callbacks.
 */
const EntityDetail = ({
  icon,
  title,
  description,
  backHref,
  backLabel,
  status,
  fields,
  actions,
  error,
  children,
}: EntityDetailProps) => {
  const { t } = useTranslation();
  const confirmTriggerRef = useRef<HTMLButtonElement>(null);
  // Snapshot captured when the dialog opens: title, description and `onConfirm`
  // are frozen so a re-render of the same key cannot retarget an open
  // destructive confirmation onto a different entity.
  const [confirming, setConfirming] = useState<{
    key: string;
    confirm: NonNullable<EntityDetailAction["confirm"]>;
  } | null>(null);
  // Live lookup by the frozen key supplies only the current `allowed`/`disabled`
  // (pending) presence; the payload and callback stay the snapshot.
  const currentAction = confirming
    ? actions.find((action) => action.key === confirming.key)
    : undefined;
  const confirmDisabled =
    !currentAction || !currentAction.allowed || currentAction.disabled === true;

  const gridFields: DefinitionGridField[] = [
    ...fields,
    ...(status ? [{ label: t("status"), value: <StatusChip status={status.tone} label={status.label} /> }] : []),
  ];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Button asChild variant="ghost" className="self-start">
        <Link to={backHref}>
          <ArrowLeft className="size-4" />
          {backLabel ?? t("back")}
        </Link>
      </Button>
      <PageChrome
        title={title}
        description={description}
        icon={icon}
        actions={
          <>
            {actions
              .filter((action) => action.allowed)
              .map((action) => {
                const Icon = action.icon;
                if (action.href) {
                  return (
                    <Button asChild variant={action.variant ?? "default"} key={action.key}>
                      <Link to={action.href}>
                        {Icon && <Icon className="size-4" />}
                        {action.label}
                      </Link>
                    </Button>
                  );
                }
                return (
                  <Button
                    key={action.key}
                    variant={action.variant ?? "default"}
                    disabled={action.disabled}
                    ref={action.confirm ? confirmTriggerRef : undefined}
                    onClick={action.confirm ? () => setConfirming({ key: action.key, confirm: action.confirm! }) : action.onClick}
                  >
                    {Icon && <Icon className="size-4" />}
                    {action.label}
                  </Button>
                );
              })}
          </>
        }
      />
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
      <SurfaceCard className="p-4">
        <DefinitionGrid fields={gridFields} />
        {children}
      </SurfaceCard>
      <ConfirmDeleteDialog
        open={confirming !== null}
        onOpenChange={(open) => {
          if (!open) setConfirming(null);
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          confirmTriggerRef.current?.focus();
        }}
        title={confirming?.confirm.title}
        description={confirming?.confirm.description}
        cancelLabel={t("cancel")}
        confirmLabel={confirming?.confirm.confirmLabel ?? t("delete")}
        confirmProps={{
          disabled: confirmDisabled,
          onClick: () => {
            if (!confirmDisabled) confirming?.confirm.onConfirm();
          },
        }}
      />
    </div>
  );
};

export default EntityDetail;
