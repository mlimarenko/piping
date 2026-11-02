import type { OperatorName } from "@/shared/api/types";

// Args sub-components
import IdentityArgs from "./args/IdentityArgs";
import LiteralArgs from "./args/LiteralArgs";
import ConcatArgs from "./args/ConcatArgs";
import SplitIndexArgs from "./args/SplitIndexArgs";
import TrimArgs from "./args/TrimArgs";
import LowerArgs from "./args/LowerArgs";
import UpperArgs from "./args/UpperArgs";
import CoerceArgs from "./args/CoerceArgs";
import LookupArgs from "./args/LookupArgs";
import TemplateArgs from "./args/TemplateArgs";
import DereferenceArgs from "./args/DereferenceArgs";
import DateFormatArgs from "./args/DateFormatArgs";
import TagsMergeArgs from "./args/TagsMergeArgs";
import MacNormalizeArgs from "./args/MacNormalizeArgs";
import IpV4PrefixCanonicalArgs from "./args/IpV4PrefixCanonicalArgs";

/**
 * Dynamic dispatcher: renders the correct args sub-component for each operator.
 * Each sub-component owns its own zod schema and inline validation.
 *
 * Follow-up: thread validation errors from parent form context down to each sub-component
 */

export interface ArgsFormProps {
  args: Record<string, unknown>;
  onChange: (args: Record<string, unknown>) => void;
  disabled?: boolean;
  /** Source entity for field-path autocomplete. */
  sourceEntity?: string;
  /** Known field paths on the source entity for autocomplete suggestions. */
  entityFieldPaths?: string[];
}

interface OperatorArgsFormProps extends ArgsFormProps {
  operator: OperatorName;
}

const OperatorArgsForm = ({
  operator,
  args,
  onChange,
  disabled,
  sourceEntity,
  entityFieldPaths,
}: OperatorArgsFormProps) => {
  const props: ArgsFormProps = { args, onChange, disabled, sourceEntity, entityFieldPaths };

  switch (operator) {
    case "identity":
      return <IdentityArgs {...props} />;
    case "literal":
      return <LiteralArgs {...props} />;
    case "concat":
      return <ConcatArgs {...props} />;
    case "split_index":
      return <SplitIndexArgs {...props} />;
    case "trim":
      return <TrimArgs {...props} />;
    case "lower":
      return <LowerArgs {...props} />;
    case "upper":
      return <UpperArgs {...props} />;
    case "coerce":
      return <CoerceArgs {...props} />;
    case "lookup":
      return <LookupArgs {...props} />;
    case "template":
      return <TemplateArgs {...props} />;
    case "dereference":
      return <DereferenceArgs {...props} />;
    case "date_format":
      return <DateFormatArgs {...props} />;
    case "tags_merge":
      return <TagsMergeArgs {...props} />;
    case "mac_normalize":
      return <MacNormalizeArgs {...props} />;
    case "ipv4_prefix_canonical":
      return <IpV4PrefixCanonicalArgs {...props} />;
    default:
      return (
        <p className="text-xs text-muted-foreground">
          Unknown operator: <span className="font-mono">{operator}</span>
        </p>
      );
  }
};

export default OperatorArgsForm;
