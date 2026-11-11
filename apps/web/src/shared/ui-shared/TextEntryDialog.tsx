import { useId, useState, type FormEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";

export interface TextEntryDialogProps {
  title: ReactNode;
  /** Names the single text field; also the dialog's accessible description. */
  label: ReactNode;
  /** Draft the dialog opens with — a rename hands in the current path. */
  defaultValue?: string;
  submitLabel: ReactNode;
  onOpenChange: (open: boolean) => void;
  /**
   * Applies the submitted text. Return an error message to keep the dialog
   * open with the typed draft and that message shown as the field error;
   * return nothing when the caller accepted the value.
   */
  onSubmit: (value: string) => string | undefined;
}

/**
 * Single-line text entry in the shared modal form: Enter submits, Escape
 * cancels, and the opening draft is focused with its text selected.
 *
 * Mount it only while the entry is requested. Dialog state (draft and error)
 * then belongs to one opening, so a rejected value keeps the user's draft and
 * the next opening starts from the caller's `defaultValue` without an effect.
 */
export default function TextEntryDialog({
  title,
  label,
  defaultValue = "",
  submitLabel,
  onOpenChange,
  onSubmit,
}: TextEntryDialogProps) {
  const { t } = useTranslation();
  const fieldId = useId();
  const [value, setValue] = useState(defaultValue);
  const [error, setError] = useState("");

  const close = () => onOpenChange(false);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!value.trim()) {
      setError(t("required"));
      return;
    }
    const message = onSubmit(value);
    if (message) {
      setError(message);
      return;
    }
    close();
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent description={label}>
        <form className="grid min-w-0 gap-4" onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
          </DialogHeader>
          <FormField label={label} htmlFor={fieldId} required error={error || undefined}>
            <FormFieldControl>
              <Input
                id={fieldId}
                autoFocus
                className="font-mono"
                value={value}
                onChange={(event) => {
                  setValue(event.target.value);
                  if (error) setError("");
                }}
                onFocus={(event) => event.currentTarget.select()}
              />
            </FormFieldControl>
          </FormField>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>
              {t("cancel")}
            </Button>
            <Button type="submit">
              {submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
