import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { CommandGroup, CommandItem } from "@/shared/ui-primitives/command";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/shared/ui-primitives/tooltip";
import { Button } from "@/shared/ui-primitives/button";
import { ChevronsUpDown, Check } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import {
  CommandPickerContent,
  CommandPickerInline,
} from "@/shared/ui-shared/CommandPicker";
import type { OperatorName, InvertibilityHint } from "@/shared/api/types";
import { listOperators, type OperatorMeta } from "@/features/inventory/sync-mapping-api";

interface OperatorPickerProps {
  value: OperatorName;
  onChange: (op: OperatorName) => void;
  disabled?: boolean;
  slot: "forward" | "reverse";
}

// Static fallback when API is unavailable
const STATIC_OPERATORS: OperatorMeta[] = [
  {
    name: "identity",
    label: "identity",
    description: "Pass field value unchanged",
    category: "identity_literal",
    invertibility_hint: "invertible",
    args_schema: {},
  },
  {
    name: "literal",
    label: "literal",
    description: "Constant value",
    category: "identity_literal",
    invertibility_hint: "forward_only",
    args_schema: {},
  },
  {
    name: "concat",
    label: "concat",
    description: "Join multiple fields or literals",
    category: "string",
    invertibility_hint: "forward_only",
    args_schema: {},
  },
  {
    name: "split_index",
    label: "split_index",
    description: "Extract segment by split",
    category: "string",
    invertibility_hint: "invertible",
    args_schema: {},
  },
  {
    name: "trim",
    label: "trim",
    description: "Remove leading/trailing whitespace",
    category: "string",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "lower",
    label: "lower",
    description: "Lowercase string",
    category: "string",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "upper",
    label: "upper",
    description: "Uppercase string",
    category: "string",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "template",
    label: "template",
    description: "String interpolation with {{var}}",
    category: "string",
    invertibility_hint: "forward_only",
    args_schema: {},
  },
  {
    name: "coerce",
    label: "coerce",
    description: "Type cast field value",
    category: "type",
    invertibility_hint: "invertible",
    args_schema: {},
  },
  {
    name: "lookup",
    label: "lookup",
    description: "Key/value table substitution",
    category: "lookup",
    invertibility_hint: "invertible",
    args_schema: {},
  },
  {
    name: "dereference",
    label: "dereference",
    description: "Resolve FK to attribute",
    category: "reference",
    invertibility_hint: "forward_only",
    args_schema: {},
  },
  {
    name: "date_format",
    label: "date_format",
    description: "Format date field",
    category: "normalization",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "tags_merge",
    label: "tags_merge",
    description: "Union two tag arrays",
    category: "normalization",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "mac_normalize",
    label: "mac_normalize",
    description: "Normalize MAC address format",
    category: "normalization",
    invertibility_hint: "normalization",
    args_schema: {},
  },
  {
    name: "ipv4_prefix_canonical",
    label: "ipv4_prefix_canonical",
    description: "Canonicalize IPv4 prefix",
    category: "normalization",
    invertibility_hint: "normalization",
    args_schema: {},
  },
];

const CATEGORY_LABELS: Record<OperatorMeta["category"], string> = {
  identity_literal: "Identity & Literal",
  string: "String",
  type: "Type",
  lookup: "Lookup",
  reference: "Reference",
  normalization: "Normalization",
};

const CATEGORY_ORDER: OperatorMeta["category"][] = [
  "identity_literal",
  "string",
  "type",
  "lookup",
  "reference",
  "normalization",
];

// ── Invertibility chip ────────────────────────────────────────────────────────

const INVERTIBILITY_CONFIG: Record<
  InvertibilityHint,
  { label: string; className: string; tooltip: string }
> = {
  invertible: {
    label: "↔ invertible",
    className: "bg-status-positive/15 text-status-positive border-status-positive/30",
    tooltip: "Can be automatically reversed. A matching reverse operator is available.",
  },
  forward_only: {
    label: "→ forward-only",
    className: "bg-muted text-muted-foreground border-border",
    tooltip: "One-way transform only. Reverse must be authored manually if needed.",
  },
  normalization: {
    label: "⇄ normalization",
    className: "bg-status-info/15 text-status-info border-status-info/30",
    tooltip:
      "Normalizing transform (lossy). Round-trip fidelity is not guaranteed; use with caution on bidirectional mappings.",
  },
};

const InvertibilityChip = ({ hint }: { hint: InvertibilityHint }) => {
  const cfg = INVERTIBILITY_CONFIG[hint];
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={cn(
              "inline-flex items-center px-1.5 py-0.5 rounded text-xs font-semibold border shrink-0 cursor-default",
              cfg.className,
            )}
          >
            {cfg.label}
          </span>
        </TooltipTrigger>
        <TooltipContent side="right" className="max-w-xs text-xs">
          {cfg.tooltip}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
};

// ── OperatorPicker ────────────────────────────────────────────────────────────

const OperatorPicker = ({
  value,
  onChange,
  disabled = false,
  slot,
}: OperatorPickerProps) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const { data: apiOperators } = useQuery({
    queryKey: ["sync", "operators"],
    queryFn: listOperators,
    staleTime: 5 * 60_000,
    retry: false,
  });

  const operators = apiOperators ?? STATIC_OPERATORS;

  const grouped = CATEGORY_ORDER.reduce<Record<string, OperatorMeta[]>>((acc, cat) => {
    acc[cat] = operators.filter((op) => op.category === cat);
    return acc;
  }, {});

  const selectedOp = operators.find((op) => op.name === value);

  return (
    <CommandPickerInline
      open={open && !disabled}
      onOpenChange={setOpen}
      placement="overlay"
      panelClassName="w-80 surface border-border"
      trigger={(
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open && !disabled}
          aria-haspopup="listbox"
          aria-label={t(
            "sync.mapping.operatorPickerLabel",
            "Select operator for {{slot}} transform",
            { slot },
          )}
          disabled={disabled}
          className="h-8 w-full justify-between border-border font-mono text-xs"
          onClick={() => setOpen((current) => !current)}
        >
          <span className="flex min-w-0 items-center gap-2">
            <span className="truncate">{selectedOp?.label ?? value}</span>
            {selectedOp && <InvertibilityChip hint={selectedOp.invertibility_hint} />}
          </span>
          <ChevronsUpDown className="ml-2 h-3 w-3 shrink-0 text-muted-foreground" />
        </Button>
      )}
    >
      <CommandPickerContent
        searchPlaceholder={t("sync.mapping.operatorPickerPlaceholder", "Search operators…")}
        emptyText={t("sync.mapping.operatorNotFound", "No operator found.")}
        listClassName="max-h-72"
        inputAutoFocus
      >
        {CATEGORY_ORDER.map((cat) => {
          const items = grouped[cat] ?? [];
          if (items.length === 0) return null;
          return (
            <CommandGroup
              key={cat}
              heading={
                <span className="px-2 text-sm font-semibold text-foreground">
                  {CATEGORY_LABELS[cat]}
                </span>
              }
            >
              {items.map((op) => (
                <CommandItem
                  key={op.name}
                  value={op.name}
                  onSelect={() => {
                    onChange(op.name);
                    setOpen(false);
                  }}
                  className="flex cursor-pointer items-start gap-2 px-3 py-1.5"
                >
                  <Check
                    className={cn(
                      "mt-0.5 h-3 w-3 shrink-0",
                      value === op.name ? "opacity-100" : "opacity-0",
                    )}
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="font-mono text-xs text-foreground">{op.label}</span>
                      <InvertibilityChip hint={op.invertibility_hint} />
                    </div>
                    <p className="mt-0.5 truncate text-xs text-muted-foreground">
                      {op.description}
                    </p>
                  </div>
                </CommandItem>
              ))}
            </CommandGroup>
          );
        })}
      </CommandPickerContent>
    </CommandPickerInline>
  );
};

export default OperatorPicker;
