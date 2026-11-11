import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import EntityFormBody from "./EntityFormBody";
import FormSection from "@/shared/ui-shared/FormSection";
import { customFieldEntityKind } from "@/features/inventory/api";
import { customFieldContentTypeTokenFor } from "@/features/inventory/tableConfig";
import EntityCustomFieldsSection, {
  type EntityCustomFieldsHandle,
} from "@/features/inventory/components/custom-fields/EntityCustomFieldsSection";
import type { EntityCustomFieldDraft } from "@/features/inventory/components/custom-fields/entityCustomFieldDraft";

/** All inventory create hosts reuse the edit authoring owner and its validation.
 * The stack, not a resource-specific editor, owns drafts across nested creation. */
export default function EntityCreateFormBody({ customFields, onCustomFields, onRegisterCustomFields, ...props }: ComponentProps<typeof EntityFormBody> & {
  customFields: EntityCustomFieldDraft;
  onCustomFields: (update: (previous: EntityCustomFieldDraft) => EntityCustomFieldDraft) => void;
  onRegisterCustomFields: (handle: EntityCustomFieldsHandle | null) => void;
}) {
  const { t } = useTranslation();
  const kind = customFieldEntityKind(props.entity);
  return (
    <div className="flex flex-col gap-6">
      <EntityFormBody {...props} />
      {kind && (
        <FormSection title={t("customFields.entity.section")}>
          <EntityCustomFieldsSection
            entityKind={kind}
            customFields={undefined}
            creationScopeToken={customFieldContentTypeTokenFor(props.entity) ?? ""}
            draftState={customFields}
            onDraftChange={onCustomFields}
            onRegister={onRegisterCustomFields}
          />
        </FormSection>
      )}
    </div>
  );
}
