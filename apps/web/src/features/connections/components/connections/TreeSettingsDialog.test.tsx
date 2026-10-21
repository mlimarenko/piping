import { fireEvent, render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { vi } from "vitest";

import TreeSettingsDialog from "./TreeSettingsDialog";
import { DEFAULT_TREE_CONFIG } from "./treeConfig";

describe("TreeSettingsDialog", () => {
  it("persists quick access protocol choices through setQuickAccess", () => {
    const setQuickAccess = vi.fn();

    render(
      <TreeSettingsDialog
        open
        onOpenChange={() => {}}
        config={DEFAULT_TREE_CONFIG}
        autoOrder={DEFAULT_TREE_CONFIG.order}
        setMode={() => {}}
        addGroup={() => {}}
        removeGroup={() => {}}
        moveGroup={() => {}}
        setIncludeUnknown={() => {}}
        setQuickAccess={setQuickAccess}
        reset={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "RDP" }));

    expect(setQuickAccess).toHaveBeenCalledWith({
      ...DEFAULT_TREE_CONFIG.quickAccess,
      protocols: ["ssh", "playbook", "rdp"],
    });
  });
});
