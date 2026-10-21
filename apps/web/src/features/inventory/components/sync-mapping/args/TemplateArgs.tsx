import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * template(pattern, vars...) — string interpolation with {{var}} tokens.
 * Args: { pattern: string, vars: Record<string, string> }
 * Variable names are auto-detected from {{...}} tokens in the pattern.
 * Follow-up: syntax highlight {{var}} tokens in pattern input
 */

const schema = z.object({
  pattern: z.string().min(1, "Pattern is required"),
  vars: z.record(z.string(), z.string()),
});

function extractVars(pattern: string): string[] {
  const matches = pattern.matchAll(/\{\{(\w+)\}\}/g);
  const seen = new Set<string>();
  const result: string[] = [];
  for (const m of matches) {
    const captured = m[1];
    if (captured === undefined) continue;
    if (!seen.has(captured)) {
      seen.add(captured);
      result.push(captured);
    }
  }
  return result;
}

const TemplateArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const pattern = (args.pattern as string | undefined) ?? "";
  const vars: Record<string, string> = (args.vars as Record<string, string> | undefined) ?? {};

  const detectedVars = extractVars(pattern);

  const result = schema.safeParse({ pattern, vars });
  const patternError = !result.success
    ? result.error.issues.find((i) => i.path[0] === "pattern")?.message
    : undefined;

  const handlePatternChange = (newPattern: string) => {
    const newVars = extractVars(newPattern).reduce<Record<string, string>>(
      (acc, v) => ({ ...acc, [v]: vars[v] ?? "" }),
      {},
    );
    onChange({ ...args, pattern: newPattern, vars: newVars });
  };

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="template-pattern"
        label={t("sync.mapping.args.pattern", "Pattern")}
        required
        error={patternError}
      >
        <Input
          id="template-pattern"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="{{name}} - {{site.name}}"
          value={pattern}
          disabled={disabled}
          onChange={(e) => handlePatternChange(e.target.value)}
        />
      </FormField>

      {detectedVars.length > 0 && (
        <div className="space-y-1.5">
          <div className="text-xs font-medium text-muted-foreground">
            {t("sync.mapping.args.variables", "Variables")}
          </div>
          {detectedVars.map((varName) => (
            <div key={varName} className="flex items-center gap-2">
              <span className="text-xs font-mono text-muted-foreground w-24 shrink-0 truncate">
                {varName}
              </span>
              <span className="text-muted-foreground text-xs shrink-0">→</span>
              <Input
                id={`template-var-${varName}`}
                className="h-9 text-sm font-mono border-input/30 flex-1"
                placeholder={varName}
                value={vars[varName] ?? ""}
                disabled={disabled}
                onChange={(e) =>
                  onChange({ ...args, vars: { ...vars, [varName]: e.target.value } })
                }
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

export default TemplateArgs;
