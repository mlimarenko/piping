import { useCallback, useReducer } from "react";
import { buildEntityDraft, type EntityFormValues } from "@/shared/forms/entityForm";
import type { InventoryEntityKey } from "@/shared/api/types";

const EMPTY_INITIAL_VALUES: Partial<EntityFormValues> = {};

/** One pending entity in the create stack. */
export interface EntityCreateFrame {
  entity: InventoryEntityKey;
  draft: EntityFormValues;
  errors: Record<string, string>;
  /** FK key on the PARENT frame's draft that receives the created id when this
   * frame resolves. Empty for the root frame. */
  returnToFieldKey: string;
}

type StackAction =
  | { type: "PUSH"; entity: InventoryEntityKey; returnToFieldKey: string }
  | { type: "POP" }
  | { type: "SET_FIELD"; key: string; value: string }
  | { type: "SET_ERRORS"; errors: Record<string, string> }
  | { type: "RESOLVE"; createdId: string }
  | { type: "RESET"; rootEntity: InventoryEntityKey; initialValues?: Partial<EntityFormValues> };

const buildSeededDraft = (
  entity: InventoryEntityKey,
  initialValues: Partial<EntityFormValues>,
): EntityFormValues => {
  const draft = buildEntityDraft(entity);
  Object.entries(initialValues).forEach(([key, value]) => {
    if (typeof value === "string") draft[key] = value;
  });
  return draft;
};

const makeFrame = (
  entity: InventoryEntityKey,
  returnToFieldKey: string,
  initialValues: Partial<EntityFormValues> = EMPTY_INITIAL_VALUES,
): EntityCreateFrame => ({
  entity,
  draft: buildSeededDraft(entity, initialValues),
  errors: {},
  returnToFieldKey,
});

/** Non-empty by invariant: the reducer always keeps at least the root frame. */
const peek = (state: EntityCreateFrame[]): EntityCreateFrame =>
  state.at(-1) as EntityCreateFrame;

const reducer = (state: EntityCreateFrame[], action: StackAction): EntityCreateFrame[] => {
  switch (action.type) {
    case "RESET":
      return [makeFrame(action.rootEntity, "", action.initialValues)];
    case "PUSH":
      return [...state, makeFrame(action.entity, action.returnToFieldKey)];
    case "POP":
      return state.length > 1 ? state.slice(0, -1) : state;
    case "SET_FIELD": {
      const top = peek(state);
      const nextErrors = { ...top.errors };
      delete nextErrors[action.key];
      const next: EntityCreateFrame = {
        ...top,
        draft: { ...top.draft, [action.key]: action.value },
        errors: nextErrors,
      };
      return [...state.slice(0, -1), next];
    }
    case "SET_ERRORS": {
      const top = peek(state);
      return [...state.slice(0, -1), { ...top, errors: action.errors }];
    }
    case "RESOLVE": {
      // Pop the resolved (top) frame and write its created id into the parent's
      // returning FK field. The root frame has no parent, so it is a no-op.
      if (state.length <= 1) return state;
      const resolved = peek(state);
      const rest = state.slice(0, -1);
      const parent = peek(rest);
      const updatedParent: EntityCreateFrame = {
        ...parent,
        draft: { ...parent.draft, [resolved.returnToFieldKey]: action.createdId },
      };
      return [...rest.slice(0, -1), updatedParent];
    }
    default:
      return state;
  }
};

export interface EntityCreateStack {
  stack: EntityCreateFrame[];
  /** The active (top) frame. */
  top: EntityCreateFrame;
  depth: number;
  reset: (rootEntity: InventoryEntityKey, initialValues?: Partial<EntityFormValues>) => void;
  push: (entity: InventoryEntityKey, returnToFieldKey: string) => void;
  pop: () => void;
  setField: (key: string, value: string) => void;
  setErrors: (errors: Record<string, string>) => void;
  resolve: (createdId: string) => void;
}

/**
 * The recursive inline-create state used by the inventory create flows: a
 * stack of drafts where each related-entity "+" pushes a frame and a successful
 * child create pops it, writing the new id into the parent's FK field. Shared by
 * the page editor and the modal host so both drive one implementation.
 */
export const useEntityCreateStack = (
  rootEntity: InventoryEntityKey,
  initialValues: Partial<EntityFormValues> = EMPTY_INITIAL_VALUES,
): EntityCreateStack => {
  const [stack, dispatch] = useReducer(reducer, [makeFrame(rootEntity, "", initialValues)]);

  const reset = useCallback(
    (entity: InventoryEntityKey, values?: Partial<EntityFormValues>) =>
      dispatch({ type: "RESET", rootEntity: entity, initialValues: values }),
    [],
  );
  const push = useCallback(
    (entity: InventoryEntityKey, returnToFieldKey: string) =>
      dispatch({ type: "PUSH", entity, returnToFieldKey }),
    [],
  );
  const pop = useCallback(() => dispatch({ type: "POP" }), []);
  const setField = useCallback(
    (key: string, value: string) => dispatch({ type: "SET_FIELD", key, value }),
    [],
  );
  const setErrors = useCallback(
    (errors: Record<string, string>) => dispatch({ type: "SET_ERRORS", errors }),
    [],
  );
  const resolve = useCallback(
    (createdId: string) => dispatch({ type: "RESOLVE", createdId }),
    [],
  );

  return {
    stack,
    top: peek(stack),
    depth: stack.length,
    reset,
    push,
    pop,
    setField,
    setErrors,
    resolve,
  };
};

/** Heuristic for self-referential parent FK fields (Tenant Group / Region /
 * Contact Group / Location parent). Used by the depth guard. */
export const DEPTH_GUARD_THRESHOLD = 5;
