import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import {
  DEPTH_GUARD_THRESHOLD,
  useEntityCreateStack,
} from "@/shared/forms/entityCreateStack";
import {
  draftToResource,
  buildEntityDraft,
  validateEntityDraft,
  type EntityFormValues,
} from "@/shared/forms/entityForm";
import type { EntityPickerOption } from "@/shared/forms/EntityPicker";
import { type InventoryEntityKey, type Resource } from "@/shared/api/types";
import { tabConfig } from "@/features/inventory/components/inventory/inventoryTypes";

const singularOf = (entity: InventoryEntityKey): string =>
  tabConfig.find((cfg) => cfg.key === entity)?.singular || entity;

const EMPTY_INITIAL_VALUES: Partial<EntityFormValues> = {};

/** Returns the localized singular display name for an entity, falling back to
 * the English singular if the translation key is missing (e.g. during tests). */
const useSingularLabel = (t: (key: string, opts?: Record<string, unknown>) => string) =>
  (entity: InventoryEntityKey): string =>
    t(`inventory.resources.entities.${entity}.one`, {
      defaultValue: singularOf(entity),
    });

interface EntityCreateEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  rootEntity: InventoryEntityKey;
  /** Optional seed values for the root create frame. Nested create frames always
   * start blank so a child form cannot inherit unrelated parent fields. */
  initialValues?: Partial<EntityFormValues>;
  /** Persists a resource and returns the created Resource (with backend id), or
   * null on failure. */
  createEntity: (entity: InventoryEntityKey, resource: Resource) => Promise<Resource | null>;
  /** Returns selectable options for a related entity. */
  loadOptions: (entity: InventoryEntityKey) => EntityPickerOption[];
  /** Reports related entities visible in the currently active create frame. */
  onVisibleRelatedEntitiesChange?: (entities: InventoryEntityKey[]) => void;
  /** Loading state for a related entity's option list. */
  isOptionsLoading?: (entity: InventoryEntityKey) => boolean;
  /** Called with the persisted root Resource after the root frame resolves. */
  onRootCreated: (created: Resource) => void;
}

const EntityCreateEditor = ({
  open,
  onOpenChange,
  rootEntity,
  initialValues = EMPTY_INITIAL_VALUES,
  createEntity,
  loadOptions,
  onVisibleRelatedEntitiesChange,
  isOptionsLoading,
  onRootCreated,
}: EntityCreateEditorProps) => {
  const { t } = useTranslation();
  const entityLabel = useSingularLabel(t);
  const { stack, top, depth, reset, push, pop, setField, setErrors, resolve } =
    useEntityCreateStack(rootEntity, initialValues);
  const [submitting, setSubmitting] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || !(event.target instanceof Element)) return;
      // Only this page owns the key. Portalled pickers and discard dialogs keep
      // their own Escape behavior. Click the canonical footer navigation action
      // so the page's dirty/saving guard also applies to keyboard navigation.
      const host = event.target.closest("[data-entity-editor-host]");
      if (!host || !cancelRef.current || !host.contains(cancelRef.current)) return;
      event.preventDefault();
      cancelRef.current.click();
    };
    document.addEventListener("keydown", onEscape);
    return () => document.removeEventListener("keydown", onEscape);
  }, [open]);

  // Reset the stack whenever the dialog opens (or the root entity changes while
  // it is already open). The initial useReducer state is frozen at mount so we
  // cannot rely on it when rootEntity changes across tab switches.
  useEffect(() => {
    if (open) reset(rootEntity, initialValues);
  }, [open, rootEntity, initialValues, reset]);

  const handleOpenChange = (v: boolean) => {
    if (v) reset(rootEntity, initialValues);
    onOpenChange(v);
  };

  const dirty = JSON.stringify(top.draft) !== JSON.stringify({ ...buildEntityDraft(top.entity), ...(depth === 1 ? initialValues : {}) });

  const handleRequestCreate = (entity: InventoryEntityKey, returnToFieldKey: string) => {
    push(entity, returnToFieldKey);
  };

  const handleBack = () => {
    pop();
  };

  const handleCreate = async () => {
    const errs = validateEntityDraft(top.entity, top.draft, t);
    if (Object.keys(errs).length > 0) {
      setErrors(errs);
      return;
    }
    setSubmitting(true);
    try {
      const resource = draftToResource(top.entity, top.draft, entityLabel(top.entity));
      const created = await createEntity(top.entity, resource);
      if (!created) return;
      if (depth > 1) {
        resolve(created.id);
      } else {
        onRootCreated(created);
        onOpenChange(false);
      }
    } finally {
      setSubmitting(false);
    }
  };

   // Page navigation pops one level while nested and closes only at the root.
  const handleScrollableOpenChange = (v: boolean) => {
    if (!v && depth > 1) {
      handleBack();
      return;
    }
    handleOpenChange(v);
  };

  const breadcrumb = stack
    .map((frame, idx) =>
      idx === 0
        ? entityLabel(frame.entity)
        : t("inventory.resources.form.createTitle", { entity: entityLabel(frame.entity) }),
    )
    .join(" › ");

  return (
    <EntityEditorHost
      open={open}
      dirty={dirty}
      saving={submitting}
      onOpenChange={handleScrollableOpenChange}
        title={
          <span className="flex items-center gap-1.5 text-sm font-medium">
            {stack.map((frame, idx) => (
              <span key={`${frame.entity}-${idx}`} className="flex items-center gap-1.5">
                {idx > 0 && <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />}
                <span className={idx === stack.length - 1 ? "text-foreground" : "text-muted-foreground"}>
                  {idx === 0
                    ? entityLabel(frame.entity)
                    : t("inventory.resources.form.createTitle", { entity: entityLabel(frame.entity) })}
                </span>
              </span>
            ))}
          </span>
        }
        description={breadcrumb}
        footer={
          <>
            {depth > 1 ? (
                <Button ref={cancelRef} data-editor-cancel variant="outline" size="sm" onClick={handleBack} disabled={submitting}>
                {t("back")}
              </Button>
            ) : (
                <Button ref={cancelRef} data-editor-cancel variant="outline" size="sm" onClick={() => onOpenChange(false)} disabled={submitting}>
                {t("cancel")}
              </Button>
            )}
            <Button size="sm" onClick={() => void handleCreate()} disabled={submitting}>
              {t("create")}
            </Button>
          </>
        }
      >
        {depth >= DEPTH_GUARD_THRESHOLD && (
          <p className="mb-3 rounded-md border border-status-warning/40 bg-status-warning/10 px-2 py-1.5 text-xs text-status-warning">
            {t("inventory.resources.form.depthWarning", {
              defaultValue: "You are nesting many create forms — consider finishing them first.",
            })}
          </p>
        )}
        <EntityFormBody
          entity={top.entity}
          draft={top.draft}
          errors={top.errors}
          onField={setField}
          loadOptions={loadOptions}
          onVisibleRelatedEntitiesChange={onVisibleRelatedEntitiesChange}
          isOptionsLoading={isOptionsLoading}
          onRequestCreate={handleRequestCreate}
        />
    </EntityEditorHost>
  );
};

export default EntityCreateEditor;
