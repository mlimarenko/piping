/** Imperative handle the host dialog calls on save for the local config
 *  context override editor. */
export interface EntityLocalContextHandle {
  /**
   * The `local_context_data` value to send on save, or `undefined` when the
   * operator did not touch the editor (so the stored override is left intact).
   * An empty editor returns `null` to clear the override. Throws
   * {@link LocalContextValidationError} on malformed JSON so the host can block
   * the save rather than 422 on the wire.
   */
  buildPayload: () => unknown;
}

/**
 * Thrown by {@link EntityLocalContextHandle.buildPayload} when the local config
 * context editor holds malformed JSON, so the host dialog can abort the save
 * instead of sending a body the backend would reject.
 */
export class LocalContextValidationError extends Error {
  constructor() {
    super("Local config context data is not valid JSON");
    this.name = "LocalContextValidationError";
  }
}
