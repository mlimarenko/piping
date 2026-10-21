import { useTranslation } from "react-i18next";
import type { EntityMapping, FieldMapping, Transform } from "@/shared/api/types";
import OperatorPicker from "./OperatorPicker";
import OperatorArgsForm from "./OperatorArgsForm";
import ReverseSlot from "./ReverseSlot";
import FormSection from "@/shared/ui-shared/FormSection";

/**
 * Forward + reverse two-slot expression container.
 * Rendered in the Expression tab of the editor right pane.
 */

interface ExpressionBuilderProps {
  entityMapping: EntityMapping;
  fieldMapping: FieldMapping;
  readonly?: boolean;
  onUpdate: (patch: Partial<FieldMapping>) => void;
}

const ExpressionBuilder = ({
  entityMapping,
  fieldMapping,
  readonly = false,
  onUpdate,
}: ExpressionBuilderProps) => {
  const { t } = useTranslation();

  // Derive field path suggestions from already-configured field mappings on this entity.
  const entityFieldPaths = entityMapping.field_mappings.map((fm) => fm.source_field);

  const handleForwardOperatorChange = (operator: Transform["operator"]) => {
    onUpdate({
      forward_transform: {
        operator,
        args: {},
      },
    });
  };

  const handleForwardArgsChange = (args: Record<string, unknown>) => {
    onUpdate({
      forward_transform: {
        ...fieldMapping.forward_transform,
        args,
      },
    });
  };

  const handleReverseUpdate = (reverse: Transform | null) => {
    onUpdate({ reverse_transform: reverse });
  };

  return (
    <div className="flex flex-col gap-6">
      {/* Forward transform */}
      <FormSection title={t("sync.mapping.forwardTransform", "Forward transform")}
        description={<span className="font-mono text-[11px]">{entityMapping.source_entity} → {entityMapping.target_entity}</span>}>

        <div className="space-y-2">
          <div className="text-sm font-medium">
            {t("sync.mapping.operatorLabel", "Operator")}
          </div>
          <OperatorPicker
            value={fieldMapping.forward_transform?.operator ?? "identity"}
            onChange={handleForwardOperatorChange}
            disabled={readonly}
            slot="forward"
          />
        </div>

        {fieldMapping.forward_transform && (
          <OperatorArgsForm
            operator={fieldMapping.forward_transform.operator}
            args={fieldMapping.forward_transform.args}
            onChange={handleForwardArgsChange}
            disabled={readonly}
            sourceEntity={entityMapping.source_entity}
            entityFieldPaths={entityFieldPaths}
          />
        )}
      </FormSection>

      {/* Reverse slot */}
      <ReverseSlot
        fieldMapping={fieldMapping}
        entityMapping={entityMapping}
        readonly={readonly}
        onUpdate={handleReverseUpdate}
      />
    </div>
  );
};

export default ExpressionBuilder;
