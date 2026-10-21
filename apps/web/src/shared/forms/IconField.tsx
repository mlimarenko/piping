import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ImageUp, RotateCcw, X } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  ICON_PACK,
  isSafeIconDataUrl,
  type EntityIconName,
} from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import { cn } from "@/shared/lib/utils";

const MAX_ICON_BYTES = 48 * 1024;
const ACCEPTED_TYPES = new Set(["image/png", "image/jpeg", "image/webp"]);

interface IconFieldProps {
  iconName: string;
  iconDataUrl?: string;
  onIconNameChange: (value: string) => void;
  onIconDataUrlChange: (value: string) => void;
  disabled?: boolean;
}

const IconField = ({
  iconName,
  iconDataUrl,
  onIconNameChange,
  onIconDataUrlChange,
  disabled,
}: IconFieldProps) => {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const [error, setError] = useState("");

  const selectedPackName = ICON_PACK.some((entry) => entry.name === iconName)
    ? (iconName as EntityIconName)
    : undefined;
  const hasCustom = isSafeIconDataUrl(iconDataUrl);

  const choosePackIcon = (name: EntityIconName) => {
    setError("");
    onIconNameChange(name);
    onIconDataUrlChange("");
  };

  const clearIcon = () => {
    setError("");
    onIconNameChange("");
    onIconDataUrlChange("");
  };

  const openFilePicker = () => {
    if (!disabled) inputRef.current?.click();
  };

  const handleFile = (file?: File) => {
    setError("");
    if (!file) return;
    if (!ACCEPTED_TYPES.has(file.type)) {
      setError(t("inventory.resources.form.iconField.invalidType"));
      return;
    }
    if (file.size > MAX_ICON_BYTES) {
      setError(t("inventory.resources.form.iconField.tooLarge"));
      return;
    }

    const reader = new FileReader();
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      if (!isSafeIconDataUrl(result)) {
        setError(t("inventory.resources.form.iconField.invalidFile"));
        return;
      }
      onIconDataUrlChange(result);
      onIconNameChange("");
    };
    reader.onerror = () => setError(t("inventory.resources.form.iconField.invalidFile"));
    reader.readAsDataURL(file);
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-border/60 bg-accent/20">
          <EntityIcon
            fallback="role"
            iconName={selectedPackName}
            customDataUrl={iconDataUrl}
            iconClassName="h-5 w-5 text-primary"
            className="h-5 w-5"
          />
        </div>
        <div className="flex min-w-0 flex-1 flex-wrap gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-9 gap-2 text-sm"
            onClick={openFilePicker}
            disabled={disabled}
          >
            <ImageUp className="h-3 w-3" />
            {t("inventory.resources.form.iconField.upload")}
          </Button>
          {(selectedPackName || hasCustom) && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-9 gap-2 text-sm text-muted-foreground"
              onClick={clearIcon}
              disabled={disabled}
            >
              <RotateCcw className="h-3 w-3" />
              {t("inventory.resources.form.iconField.reset")}
            </Button>
          )}
        </div>
        {hasCustom && (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t("inventory.resources.form.iconField.removeCustom")}
            onClick={() => onIconDataUrlChange("")}
            disabled={disabled}
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>

      <input
        ref={inputRef}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        className="hidden"
        disabled={disabled}
        onChange={(event) => {
          handleFile(event.currentTarget.files?.[0]);
          event.currentTarget.value = "";
        }}
      />

      <div
        role="group"
        aria-label={t("inventory.resources.form.iconField.packLabel")}
        className="grid grid-cols-6 gap-1 sm:grid-cols-8"
      >
        {ICON_PACK.map((entry) => {
          const isActive = selectedPackName === entry.name && !hasCustom;
          return (
            <button
              key={entry.name}
              type="button"
              className={cn(
                "flex h-8 w-8 items-center justify-center rounded-md border border-input/30 bg-accent/20 text-muted-foreground transition-colors",
                "hover:bg-accent/40 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                isActive && "border-primary/60 bg-primary/10 text-primary",
              )}
              aria-label={t(entry.labelKey, { defaultValue: entry.fallback })}
              aria-pressed={isActive}
              title={t(entry.labelKey, { defaultValue: entry.fallback })}
              onClick={() => choosePackIcon(entry.name)}
              disabled={disabled}
            >
              <entry.Icon className="h-4 w-4" />
            </button>
          );
        })}
      </div>

      {error && <p className="text-xs text-destructive">{error}</p>}
      <p className="text-xs text-muted-foreground">
        {t("inventory.resources.form.iconField.hint")}
      </p>
    </div>
  );
};

export default IconField;
