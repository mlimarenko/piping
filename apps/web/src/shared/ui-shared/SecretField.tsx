import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Eye, EyeOff } from "lucide-react";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";

function secretText(value: string | undefined): string {
  return typeof value === "string" ? value : "";
}

export interface SecretFieldProps {
  label: string;
  value: string | undefined;
  /** Use a resizable textarea instead of a single-line input (e.g. private key). */
  multiline?: boolean;
  onChange: (next: string | undefined) => void;
  placeholder?: string;
  /** False when the field is a secret-ref the caller may not view: hides the eye. */
  canReveal?: boolean;
  /** True while the reveal request is in flight: keep the value masked. */
  loading?: boolean;
}

/**
 * A credential field that masks its value with CSS `text-security` and offers an
 * inset eye toggle to reveal it. The value is always entered as plain text; the
 * mask is purely visual (the DOM value is untouched, so typing/paste/select-all
 * work normally).
 *
 * Reveal control:
 * - `canReveal === false` hides the eye and keeps the value permanently masked
 *   (e.g. a secret reference the caller lacks permission to view).
 * - `loading === true` masks the value while an upstream reveal request is in
 *   flight, so a placeholder/UUID is never flashed.
 *
 * Shared between the connection-config record editor (where it renders resolved
 * secret refs) and the ad-hoc credentials dialog (where it renders plain
 * literals with `canReveal=true`, `loading=false`).
 */
function SecretField({
  label,
  value,
  multiline = false,
  onChange,
  placeholder,
  canReveal = true,
  loading = false,
}: SecretFieldProps) {
  const { t } = useTranslation();
  const inputId = useId();
  const text = secretText(value);
  const [revealed, setRevealed] = useState(false);
  const isEmpty = text.length === 0;
  // The eye is only meaningful when the field has content the user is allowed
  // to unmask. A secret-ref the caller can't view is permanently masked.
  const showEye = !isEmpty && canReveal && !loading;
  const forceMasked = loading || !canReveal;

  // Shared classes for both input variants
  const fieldBase = "w-full pr-10 font-mono text-sm";
  // CSS text-security masks glyphs without altering the DOM value, so
  // typing/paste/select-all work normally. Supported in Chrome, Firefox, Safari.
  const maskedStyle: React.CSSProperties = ((forceMasked || !revealed) && !isEmpty)
    ? { WebkitTextSecurity: "disc", textSecurity: "disc" } as React.CSSProperties
    : {};

  const handleChange = (raw: string) => {
    onChange(raw.trim() ? raw : undefined);
  };

  return (
    <div className="flex min-w-0 flex-col gap-2">
      {/* Label row — the value is always entered as plain text and stored as a
          masked secret; there is no user-facing secret-reference mode. */}
      <div>
        <label htmlFor={inputId} className="text-sm font-medium">
          {label}
        </label>
      </div>

      {/* Input wrapper with inset eye-reveal button */}
      <div className="relative">
        {multiline ? (
          <Textarea
            id={inputId}
            className={`${fieldBase} resize-y`}
            style={maskedStyle}
            value={text}
            placeholder={placeholder}
            onChange={(e) => handleChange(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
        ) : (
          <Input
            id={inputId}
            className={fieldBase}
            style={maskedStyle}
            value={text}
            placeholder={placeholder}
            onChange={(e) => handleChange(e.target.value)}
            autoComplete="off"
          />
        )}

        {/* Eye reveal/hide button — hidden when masked-only (loading or no perm) */}
        {showEye && (
          <button
            type="button"
            onClick={() => setRevealed((r) => !r)}
            aria-label={revealed
              ? t("connectionConfig.fields.hideSecret")
              : t("connectionConfig.fields.revealSecret")}
            className="absolute right-1 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring"
            style={multiline ? { top: "1rem", transform: "none" } : {}}
          >
            {revealed
              ? <EyeOff className="h-3.5 w-3.5" />
              : <Eye className="h-3.5 w-3.5" />}
          </button>
        )}
      </div>
    </div>
  );
}

export default SecretField;
