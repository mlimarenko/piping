/**
 * Thrown by the entity custom-fields section's `buildPayload` when a JSON custom
 * field holds malformed JSON, so the host dialog can abort the save instead of
 * sending a body the backend would reject with a 422. `fieldLabel` names the
 * offending field for the user-facing message.
 */
export class CustomFieldValidationError extends Error {
  readonly fieldLabel: string;
  constructor(fieldLabel: string) {
    super(`Invalid custom field value: ${fieldLabel}`);
    this.name = "CustomFieldValidationError";
    this.fieldLabel = fieldLabel;
  }
}
