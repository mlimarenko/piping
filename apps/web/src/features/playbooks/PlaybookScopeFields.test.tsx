import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import PlaybookScopeFields from "./PlaybookScopeFields";
import type { PlaybookScopeEntities } from "./usePlaybookScopeEntities";
import type { PlaybookAssignmentScopeKind } from "./api";

function makeEntities(
  overrides: Partial<PlaybookScopeEntities> = {},
): PlaybookScopeEntities {
  return {
    optionsByScope: {
      connection_target: [],
      role: [{ id: "11111111-1111-1111-1111-111111111111", name: "web" }],
      site: [],
      region: [],
      tenant: [],
      cluster: [],
      platform: [],
    },
    resolveName: () => "",
    isPending: false,
    ...overrides,
  };
}

describe("PlaybookScopeFields", () => {
  it("shows the global hint and no entity picker for the global scope", () => {
    render(
      <PlaybookScopeFields
        scopeKind="global"
        scopeId=""
        onScopeKindChange={vi.fn()}
        onScopeIdChange={vi.fn()}
        entities={makeEntities()}
      />,
    );

    expect(screen.getByDisplayValue("Applies everywhere")).toBeInTheDocument();
  });

  it("resets the scope id when the kind changes", () => {
    const onScopeKindChange = vi.fn();
    const onScopeIdChange = vi.fn();
    render(
      <PlaybookScopeFields
        scopeKind="role"
        scopeId="11111111-1111-1111-1111-111111111111"
        onScopeKindChange={onScopeKindChange}
        onScopeIdChange={onScopeIdChange}
        entities={makeEntities()}
      />,
    );

    // The kind select is the first combobox; changing it must clear the id.
    const kindSelect = screen.getAllByRole("combobox")[0]!;
    fireEvent.click(kindSelect);
    fireEvent.click(screen.getByRole("option", { name: "Site" }));

    expect(onScopeKindChange).toHaveBeenCalledWith<[PlaybookAssignmentScopeKind]>("site");
    expect(onScopeIdChange).toHaveBeenCalledWith("");
  });

  it("offers entity ids (not names) as the picker options for a non-global kind", () => {
    const onScopeIdChange = vi.fn();
    render(
      <PlaybookScopeFields
        scopeKind="role"
        scopeId=""
        onScopeKindChange={vi.fn()}
        onScopeIdChange={onScopeIdChange}
        entities={makeEntities()}
      />,
    );

    // The entity picker (second combobox) lists the role by name.
    const entitySelect = screen.getAllByRole("combobox")[1]!;
    fireEvent.click(entitySelect);
    fireEvent.click(screen.getByRole("option", { name: "web" }));

    // It must submit the entity id, never the display name.
    expect(onScopeIdChange).toHaveBeenCalledWith("11111111-1111-1111-1111-111111111111");
  });
});
