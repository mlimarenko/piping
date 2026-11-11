/** Stops host submission without exposing backend diagnostics or secret values. */
export class CustomFieldValidationError extends Error {
    readonly fieldLabel: string;
  readonly reason: "json" | "required" | "invalid" | "unavailable" | "uneditable";
  constructor(fieldLabel: string, reason: CustomFieldValidationError["reason"] = "json") {
    super(`Invalid custom field value: ${fieldLabel}`);
    this.name = "CustomFieldValidationError";
    this.fieldLabel = fieldLabel;
    this.reason = reason;
  }
}

export function customFieldValidationMessage(t: (key: string, options?: Record<string, unknown>) => string, error: CustomFieldValidationError): string {
  const keys = {
    json: "invalidJsonField",
    required: "requiredField",
    invalid: "invalidField",
    unavailable: "definitionsUnavailable",
    uneditable: "requiredFieldUnavailable",
  };
  return t(`customFields.entity.${keys[error.reason]}`, { field: error.fieldLabel });
}
