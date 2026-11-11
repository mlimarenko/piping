import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EntityMapping } from "@/shared/api/types";
import FieldTreeRail from "./FieldTreeRail";

const mapping: EntityMapping = {
  id: "synthetic-device",
  source_entity: "dcim.devices",
  target_entity: "piping.inventory.device",
  enabled: true,
  field_mappings: [{
    id: "synthetic-name",
    source_field: "name",
    target_field: "name",
    enabled: true,
    forward_transform: { operator: "identity", args: {} },
    reverse_transform: null,
  }],
};

describe("FieldTreeRail entity controls", () => {
  it("adds a field with an independent keyboard-focusable action and hides it in readonly mode", () => {
    const addField = vi.fn();
    const selectEntity = vi.fn();
    const props = { entityMappings: [mapping], selectedField: null, onSelectField: vi.fn(),
      onSelectEntity: selectEntity, onAddEntity: vi.fn(), onAddField: addField };
    const { rerender } = render(<FieldTreeRail {...props} />);
    const action = screen.getByRole("button", { name: /add field/i });
    expect(action.tabIndex).toBe(0);
    action.focus();
    expect(action).toHaveFocus();
    fireEvent.click(action);
    expect(selectEntity).toHaveBeenCalledWith(mapping.id);
    expect(addField).toHaveBeenCalledWith(mapping.id);
    rerender(<FieldTreeRail {...props} readonly />);
    expect(screen.queryByRole("button", { name: /add field/i })).not.toBeInTheDocument();
  });

  it("keeps expansion and entity-selection buttons as siblings", () => {
    const { container } = render(<FieldTreeRail entityMappings={[mapping]} selectedField={null}
      onSelectField={vi.fn()} onSelectEntity={vi.fn()} onAddEntity={vi.fn()} />);
    expect(container.querySelector("button button")).toBeNull();
  });

  it("collapses a selected entity without selecting it again and expands with the keyboard", () => {
    const selectEntity = vi.fn();
    const { container } = render(<FieldTreeRail entityMappings={[mapping]} selectedEntityId={mapping.id}
      selectedField={null} onSelectField={vi.fn()} onSelectEntity={selectEntity} onAddEntity={vi.fn()} />);
    const entity = screen.getAllByRole("treeitem")[0];
    if (!entity) throw new Error("Expected the synthetic entity tree item");
    fireEvent.click(screen.getByRole("button", { name: /collapse/i }));
    expect(selectEntity).not.toHaveBeenCalled();
    expect(entity).toHaveAttribute("aria-expanded", "false");
    expect(container.querySelector('[data-tree-key="field:synthetic-device:synthetic-name"]')).toBeNull();
    fireEvent.keyDown(entity, { key: "ArrowRight" });
    expect(entity).toHaveAttribute("aria-expanded", "true");
    expect(container.querySelector('[data-tree-key="field:synthetic-device:synthetic-name"]')).not.toBeNull();
  });
});
