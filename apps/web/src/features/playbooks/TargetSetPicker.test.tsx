import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import type { PlaybookTargetSetListItem } from "./api";
import { TargetSetPicker } from "./TargetSetPicker";

const base = {
  entity_kind: "connection_target",
  selector: {},
  created_at: "",
  updated_at: "",
  usage: { job_templates: [], assignments: [], schedules: [] },
};

function targetSet(overrides: Partial<PlaybookTargetSetListItem>): PlaybookTargetSetListItem {
  return { ...base, id: "set-1", name: "Production", mode: "static", resolved_count: 3, ...overrides };
}

const sets = [
  targetSet({}),
  targetSet({ id: "set-2", name: "Backup", mode: "dynamic", resolved_count: 2, selector: { status: ["active"] } }),
  targetSet({ id: "set-3", name: "Unknown", mode: "dynamic", resolved_count: null }),
];

function openList() {
  fireEvent.click(screen.getByRole("combobox"));
}

describe("TargetSetPicker", () => {
  it("labels every option with the name, the mode and the resolved host count", () => {
    render(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} />);
    openList();
    expect(screen.getByRole("option", { name: /Production/ })).toHaveTextContent("Production · Fixed set · 3 hosts");
    expect(screen.getByRole("option", { name: /Backup/ })).toHaveTextContent("Backup · Dynamic selector · 2 hosts");
  });

  it("never reports an unknown host count as zero", () => {
    render(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} />);
    openList();
    const unknown = screen.getByRole("option", { name: /Unknown/ });
    expect(unknown).toHaveTextContent("Host count unavailable");
    expect(unknown).not.toHaveTextContent("0 hosts");
  });

  it("selects a set by id", () => {
    const onValueChange = vi.fn();
    render(<TargetSetPicker targetSets={sets} value="" onValueChange={onValueChange} />);
    openList();
    fireEvent.click(screen.getByRole("option", { name: /Backup/ }));
    expect(onValueChange).toHaveBeenCalledWith("set-2");
  });

  it.each([
    [{ kind: "ready", total: 5 } as const, "Resolves to 5 hosts"],
    [{ kind: "denied" } as const, "You don't have access to this section"],
    [{ kind: "pending" } as const, "Resolving targets..."],
    [{ kind: "error" } as const, "Target preview failed"],
  ])("renders the %o preview of the selected set", (preview, text) => {
    render(<TargetSetPicker targetSets={sets} value="set-1" onValueChange={vi.fn()} preview={preview} />);
    expect(screen.getByText(text)).toBeInTheDocument();
  });

  it("shows no preview box without a selected set or without a resolved preview", () => {
    const { rerender } = render(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} preview={{ kind: "ready", total: 5 }} />);
    expect(screen.queryByText("Resolves to 5 hosts")).toBeNull();
    rerender(<TargetSetPicker targetSets={sets} value="set-1" onValueChange={vi.fn()} preview={{ kind: "idle" }} />);
    expect(screen.queryByText("Resolves to 5 hosts")).toBeNull();
  });

  it("renders the create action only when the owner provides it", () => {
    const onCreate = vi.fn();
    const { rerender } = render(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "New target set" })).toBeNull();
    rerender(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} onCreate={onCreate} />);
    fireEvent.click(screen.getByRole("button", { name: "New target set" }));
    expect(onCreate).toHaveBeenCalledOnce();
  });

  it("renders the owner's browse action next to the create action", () => {
    const onBrowse = vi.fn();
    render(<TargetSetPicker targetSets={sets} value="" onValueChange={vi.fn()} onBrowse={onBrowse} />);
    fireEvent.click(screen.getByRole("button", { name: "Target sets" }));
    expect(onBrowse).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "New target set" })).toBeNull();
  });
});
