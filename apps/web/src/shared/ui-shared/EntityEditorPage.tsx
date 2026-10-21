import { type ReactNode } from "react";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import PageLayout from "@/shared/ui-shared/PageLayout";
import PageChrome from "./PageChrome";

/**
 * One breadcrumb entry in the editor header. `onClick` (or `to` handled by the
 * caller) navigates back to an ancestor list; the last entry is the current
 * entity and renders as plain text.
 */
export interface EntityEditorBreadcrumb {
  /** Stable identity for the React key; unique among the crumbs in a trail. */
  key: string;
  label: ReactNode;
  onClick?: () => void;
}

/**
 * Canonical entity editor shell. Routed editors portal their title and
 * breadcrumb/back actions to TopBar; embedded editors own a local h2.
 * Natural-height fields and the adjacent Save/Cancel
 * footer — so every domain editor (inventory resources, playbooks, access,
 * webhooks, …) can adopt the same structure instead of each growing its own
 * form core. It carries no form state and no data access.
 *
 */
export interface EntityEditorPageProps {
  scope: "routed" | "embedded";
  /** Primary heading (entity name / "New <entity>"). */
  title: ReactNode;
  /** Optional sub-heading under the title. */
  description?: ReactNode;
  /** Ancestor crumbs rendered between the back button and the title. */
  breadcrumbs?: EntityEditorBreadcrumb[];
  /** Back affordance behaviour (navigate to list / pop the inline create stack). */
  onBack: () => void;
  /** Accessible + visible label for the back button. */
  backLabel: string;
  /** Breadcrumb nav accessible name. */
  breadcrumbLabel?: string;
  /** Editor body. */
  children: ReactNode;
  /** Replaces the whole footer when supplied. */
  footer?: ReactNode;
  /** Default footer: Save action. Omit `onSave` for a read-only page. */
  onSave?: () => void;
  saveLabel?: string;
  saving?: boolean;
  saveDisabled?: boolean;
  /** Default footer: Cancel action; falls back to `onBack`. */
  onCancel?: () => void;
  cancelLabel?: string;
  /** When true, shows the unsaved-changes banner and registers `beforeunload`. */
  dirty?: boolean;
  /** Banner text shown while `dirty`. */
  dirtyMessage?: string;
  /** Page-level error/alerts rendered above the body. */
  error?: ReactNode;
   /** Canonical page width. */
  width?: "standard" | "form" | "compact";
  className?: string;
  bodyClassName?: string;
  testId?: string;
}

const EntityEditorPage = ({
  scope,
  title,
  description,
  breadcrumbs,
  onBack,
  backLabel,
  breadcrumbLabel,
  children,
  footer,
  onSave,
  saveLabel = "Save",
  saving = false,
  saveDisabled = false,
  onCancel,
  cancelLabel = "Cancel",
  dirty = false,
  dirtyMessage,
  error,
  width = "form",
  className,
  bodyClassName,
  testId = "entity-editor-page",
}: EntityEditorPageProps) => {
  const { allowExit, confirmExit, discardDialog } = useUnsavedChangesGuard(dirty, saving);

  const navigation = <>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="gap-1.5 text-sm text-muted-foreground"
          disabled={saving}
          onClick={() => { if (dirty && !allowExit.current) confirmExit(onBack); else onBack(); }}
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          {backLabel}
        </Button>

        {breadcrumbs && breadcrumbs.length > 0 && (
          <nav
            aria-label={breadcrumbLabel ?? backLabel}
            className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground"
          >
            {breadcrumbs.map((crumb, index) => (
              <span key={crumb.key} className="flex min-w-0 items-center gap-1.5">
                {index > 0 && <span aria-hidden>›</span>}
                {crumb.onClick ? (
                  <button
                    type="button"
                    disabled={saving}
                    className="truncate rounded-sm underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={() => {
                      const action = crumb.onClick!;
                      if (dirty && !allowExit.current) confirmExit(action);
                      else action();
                    }}
                  >
                    {crumb.label}
                  </button>
                ) : (
                  <span className="truncate">{crumb.label}</span>
                )}
              </span>
            ))}
          </nav>
        )}

  </>;
  return (
    <PageLayout
      width={width}
      inset={false}
      data-slot="entity-editor-page"
      data-testid={testId}
      className={cn("flex min-h-0 flex-col rounded-xl border border-border/60 bg-card p-4 shadow-md md:p-8", className)}
    >
      {scope === "routed" ? <PageChrome title={title} description={description} actions={navigation} /> :
      <header className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-4 pb-6">
        {navigation}
        <div className="flex min-w-0 basis-full flex-col gap-2">
          <h2 tabIndex={-1} className="break-words text-xl font-semibold tracking-tight text-foreground outline-none">
            {title}
          </h2>
          {description && (
            <p className="max-w-prose text-sm leading-relaxed text-muted-foreground">{description}</p>
          )}
        </div>
      </header>}

      {error && <div className="shrink-0 pt-4">{error}</div>}

      {dirty && dirtyMessage && (
        <output
          className="mt-4 block rounded-md border border-status-warning/40 bg-status-warning/10 px-3 py-2 text-xs text-status-warning"
        >
          {dirtyMessage}
        </output>
      )}

      <div className={cn("@container min-w-0 pb-2", bodyClassName)}>
        {children}
      </div>

      <footer
        onClickCapture={event => {
          const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-editor-cancel]");
          if (button && saving) { event.preventDefault(); event.stopPropagation(); return; }
          if (!button || !dirty || allowExit.current) return;
          event.preventDefault(); event.stopPropagation();
          confirmExit(() => button.click());
        }}
        className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-border/60 pt-6"
      >
        {footer ?? (
          <>
            <Button
              type="button"
              variant="outline"
              onClick={() => { const action = onCancel ?? onBack; if (dirty && !allowExit.current) confirmExit(action); else action(); }}
              disabled={saving}
            >
              {cancelLabel}
            </Button>
            {onSave && (
              <Button
                type="button"
                onClick={onSave}
                disabled={saving || saveDisabled}
              >
                {saveLabel}
              </Button>
            )}
          </>
        )}
      </footer>
      {discardDialog}
    </PageLayout>
  );
};

export default EntityEditorPage;
