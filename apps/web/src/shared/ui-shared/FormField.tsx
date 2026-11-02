import { cloneElement, createContext, forwardRef, isValidElement, useContext, useId, type AriaAttributes, type ComponentPropsWithoutRef, type ReactNode } from "react";
import { Slot } from "@radix-ui/react-slot";
import { cn } from "@/shared/lib/utils";
import FieldLabel from "@/shared/ui-shared/FieldLabel";

export interface FormFieldProps {
  label?: ReactNode;
  htmlFor?: string;
  required?: boolean;
  help?: ReactNode;
  error?: ReactNode;
  children: ReactNode;
  className?: string;
  labelClassName?: string;
}

const FormFieldContext = createContext<{ descriptionId?: string; invalid: boolean } | null>(null);

/** Slot only the actual control (or an explicitly labelled group), never its layout. */
export const FormFieldControl = forwardRef<HTMLElement, ComponentPropsWithoutRef<typeof Slot>>(
  ({ children, "aria-describedby": describedBy, ...props }, ref) => {
    const field = useContext(FormFieldContext);
    if (!field) throw new Error("FormFieldControl must be used within FormField");
    if (!isValidElement<AriaAttributes>(children)) {
      throw new Error("FormFieldControl requires one control element");
    }
    const description = [...new Set(
      [children.props["aria-describedby"], describedBy, field.descriptionId]
        .filter(Boolean).join(" ").split(/\s+/).filter(Boolean),
    )].join(" ") || undefined;

    return (
      <Slot ref={ref} aria-invalid={field.invalid || undefined} {...props}>
        {cloneElement(children, {
          "aria-describedby": description,
          "aria-invalid": children.props["aria-invalid"] ?? props["aria-invalid"] ?? (field.invalid || undefined),
        })}
      </Slot>
    );
  },
);
FormFieldControl.displayName = "FormFieldControl";

const FormField = ({
  label,
  htmlFor,
  required,
  help,
  error,
  children,
  className,
  labelClassName,
}: FormFieldProps) => {
  const id = useId();
  const helpId = `${id}-help`;
  const errorId = `${id}-error`;
  const descriptionId = error ? errorId : help ? helpId : undefined;
  return (
    <FormFieldContext.Provider value={{ descriptionId, invalid: !!error }}>
      <div data-slot="form-field" className={cn("flex min-w-0 flex-col gap-2", className)}>
        {label ? (
          <FieldLabel
            htmlFor={htmlFor}
            required={required}
            className={cn("text-sm font-medium", labelClassName)}
          >
            {label}
          </FieldLabel>
        ) : null}
        {children}
        {error ? (
          <p id={errorId} role="alert" className="text-xs leading-snug text-status-negative">{error}</p>
        ) : help ? (
          <p id={helpId} className="text-xs leading-snug text-muted-foreground">{help}</p>
        ) : null}
      </div>
    </FormFieldContext.Provider>
  );
};

export default FormField;
