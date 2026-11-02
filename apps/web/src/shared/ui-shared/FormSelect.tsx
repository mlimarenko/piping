import { useId, type ReactNode } from "react";
import { cn } from "@/shared/lib/utils";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/shared/ui-primitives/select";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";

export interface FormSelectOption<TValue extends string = string> {
  value: TValue;
  label: ReactNode;
  disabled?: boolean;
}

export interface FormSelectProps<TValue extends string = string> {
  id?: string;
  value: TValue;
  onValueChange: (value: TValue) => void;
  options: FormSelectOption<TValue>[];
  label?: ReactNode;
  placeholder?: string;
  required?: boolean;
  help?: ReactNode;
  error?: ReactNode;
  disabled?: boolean;
  className?: string;
  labelClassName?: string;
  triggerClassName?: string;
  contentClassName?: string;
  itemClassName?: string;
  "aria-label"?: string;
  "aria-invalid"?: boolean;
  "aria-describedby"?: string;
}

const FormSelect = <TValue extends string = string>({
  id,
  value,
  onValueChange,
  options,
  label,
  placeholder,
  required,
  help,
  error,
  disabled,
  className,
  labelClassName,
  triggerClassName,
  contentClassName,
  itemClassName,
  ...rest
}: FormSelectProps<TValue>) => {
  const generatedId = useId();
  const fieldId = id ?? generatedId;

  return (
    <FormField
      htmlFor={fieldId}
      label={label}
      required={required}
      help={help}
      error={error}
      className={className}
      labelClassName={labelClassName}
    >
      <Select
        value={value}
        onValueChange={(next) => onValueChange(next as TValue)}
        disabled={disabled}
      >
        <FormFieldControl>
          <SelectTrigger
            id={fieldId}
            aria-label={rest["aria-label"]}
            aria-invalid={rest["aria-invalid"]}
            aria-describedby={rest["aria-describedby"]}
            className={cn("h-9 text-sm", triggerClassName)}
          >
            <SelectValue placeholder={placeholder} />
          </SelectTrigger>
        </FormFieldControl>
        <SelectContent className={contentClassName}>
          {options.map((option) => (
            <SelectItem
              key={option.value}
              value={option.value}
              disabled={option.disabled}
              className={cn("text-sm", itemClassName)}
            >
              {option.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </FormField>
  );
};

export default FormSelect;
