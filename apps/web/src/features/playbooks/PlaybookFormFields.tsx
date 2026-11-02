import { Children, cloneElement, isValidElement, useId, type ReactElement } from "react";
import { Label } from "@/shared/ui-primitives/label";
import { Switch } from "@/shared/ui-primitives/switch";
import { cn } from "@/shared/lib/utils";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { FieldHelp } from "./FieldHelp";

export function DetailField({
  label,
  children,
  className,
  mono = false,
}: Readonly<{
  label: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  mono?: boolean;
}>) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <p className="text-xs text-muted-foreground">
        {label}
      </p>
      <div
        className={cn(
          "min-w-0 break-words text-sm text-foreground",
          mono && "font-mono text-[11px]",
        )}
      >
        {children}
      </div>
    </div>
  );
}

export function FormField({
  label,
  children,
  className,
  required,
  help,
  labelClassName,
}: Readonly<{
  label: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  required?: boolean;
  help?: string;
  labelClassName?: string;
}>) {
  const fieldId = useId();
  const labelId = `${fieldId}-label`;
  const fields = Children.toArray(children);
  const control = fields[0];
  let controlId = fieldId;
  if (isValidElement(control)) {
    const element = control as ReactElement<{ id?: string; "aria-labelledby"?: string }>;
    controlId = element.props.id ?? fieldId;
    fields[0] = cloneElement(element, { id: controlId, "aria-labelledby": element.props["aria-labelledby"] ?? labelId });
  }
  return (
    <div className={cn("flex min-w-0 flex-col gap-2", className)}>
      <FieldLabel id={labelId} htmlFor={controlId} required={required} className={cn("flex items-center gap-1 text-sm font-medium", labelClassName)}>
        {label}{help && <FieldHelp text={help} />}
      </FieldLabel>
      {fields}
    </div>
  );
}

export function SectionLabel({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <h2 className="text-sm font-medium text-foreground">
      {children}
    </h2>
  );
}

export function ToggleField({
  label,
  description,
  checked,
  onCheckedChange,
  disabled = false,
}: Readonly<{
  label: string;
  description?: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
}>) {
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-4 py-2">
      <div className="min-w-0">
        <Label htmlFor={id} className="text-sm font-medium">{label}</Label>
        {description ? (
          <p className="text-xs leading-snug text-muted-foreground">{description}</p>
        ) : null}
      </div>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={onCheckedChange} />
    </div>
  );
}
