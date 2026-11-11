import { act, fireEvent, render, screen, within, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import i18n from "i18next";
import { ensureLanguageBundle } from "@/shared/i18n";
import { testUuid } from "@/test/ids";
import { Dialog, DialogContent } from "@/shared/ui-primitives/dialog";
import ConnectionsFilters from "./ConnectionsFilters";
import { emptyFilterState } from "./filters-utils";
import type { ComponentProps } from "react";

function renderFilters(overrides: Partial<ComponentProps<typeof ConnectionsFilters>> = {}) {
  const onClose = vi.fn(), onChange = vi.fn(), onApplyPreset = vi.fn();
  const onSavePreset = vi.fn();
  const view = render(<Dialog open><DialogContent><ConnectionsFilters hosts={[{ id: testUuid("target"), name: "Host", site: "Frankfurt", targetKind: "device", status: "active" }]}
    selected={emptyFilterState()} onClose={onClose} onChange={onChange} privatePresets={[{ id: "preset", name: "Active", visibility: "private", filters: { status: ["active"] }, createdAt: "", updatedAt: "" }]}
    publicPresets={[]} onApplyPreset={onApplyPreset} onSavePreset={onSavePreset} onDeletePreset={vi.fn()} {...overrides} /></DialogContent></Dialog>);
  return { onClose, onChange, onApplyPreset, onSavePreset, ...view };
}

describe("filter draft", () => {
  it("publishes only on Apply and returns to the catalogue", () => {
    const { onClose, onChange } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ site: new Set(["Frankfurt"]) }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("applies edits made after choosing a saved preset instead of the original preset", () => {
    const { onChange, onApplyPreset } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Saved filters 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /Active.*selected/ }));
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ site: new Set(["Frankfurt"]), status: new Set(["active"]) }));
    expect(onApplyPreset).not.toHaveBeenCalled();
  });

  it("loads a preset into the draft and only applies on confirmation", () => {
    const { onChange, onApplyPreset } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Saved filters 1/ }));
    fireEvent.click(screen.getByRole("button", { name: /Active.*selected/ }));
    expect(onChange).not.toHaveBeenCalled();
    expect(onApplyPreset).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApplyPreset).toHaveBeenCalledTimes(1);
  });

  it("reopening seeds from applied state, not a cancelled draft", () => {
    const { onChange, unmount } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    unmount();
    renderFilters();
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Remove Site/ })).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("saves the draft conditions without first applying them to the catalogue", async () => {
    const { onSavePreset, onChange } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Save filter" }));
    const saveDialog = screen.getByRole("dialog", { name: "Save filter" });
    fireEvent.change(within(saveDialog).getByPlaceholderText("Filter name"), { target: { value: "Frankfurt draft" } });
    fireEvent.click(within(saveDialog).getByRole("button", { name: "Save filter" }));
    await waitFor(() => expect(onSavePreset).toHaveBeenCalledWith("Frankfurt draft", "private", expect.objectContaining({ site: new Set(["Frankfurt"]) })));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("draft controls", () => {
  it("keeps Apply inert until the draft changes, then Reset makes it inert again", () => {
    const { onChange } = renderFilters();
    const apply = screen.getByRole("button", { name: "Apply" });
    expect(apply).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    expect(apply).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Reset draft" }));
    expect(apply).toBeDisabled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("Cancel discards the draft without publishing it", () => {
    const { onChange, onClose } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onChange).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("dismissing the save-filter dialog keeps the draft and does not save", async () => {
    const { onSavePreset, onChange } = renderFilters();
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Save filter" }));
    const saveDialog = screen.getByRole("dialog", { name: "Save filter" });
    fireEvent.keyDown(saveDialog, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Save filter" })).not.toBeInTheDocument(),
    );
    expect(onSavePreset).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled();
  });
});

describe("finding conditions", () => {
  it("searches fields and values and removes a contextual chip", () => {
    const { onChange } = renderFilters();
    fireEvent.change(screen.getByPlaceholderText("Search fields"), { target: { value: "missing" } });
    expect(screen.getByText("No results found")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("Search fields"), { target: { value: "site" } });
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    expect(screen.queryByPlaceholderText("Search Site values")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: /Site: Frankfurt/ }));
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("translates status without changing the applied value", async () => {
    await ensureLanguageBundle("ru");
    await i18n.changeLanguage("ru");
    try {
      const { onChange } = renderFilters();
      fireEvent.click(screen.getByRole("button", { name: "активен" }));
      fireEvent.click(screen.getByRole("button", { name: "Применить" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ status: new Set(["active"]) }));
    } finally {
      await act(() => i18n.changeLanguage("en"));
    }
  });

  it("limits rendered values while searching the full collection", () => {
    const hosts = Array.from({ length: 4000 }, (_, index) => ({
      id: testUuid(`target-${index}`), name: `Host ${String(index).padStart(4, "0")}`, targetKind: "device" as const,
    }));
    render(<Dialog open><DialogContent><ConnectionsFilters hosts={hosts} selected={emptyFilterState()}
      onClose={vi.fn()} onChange={vi.fn()} privatePresets={[]} publicPresets={[]}
      onApplyPreset={vi.fn()} onSavePreset={vi.fn()} onDeletePreset={vi.fn()} /></DialogContent></Dialog>);
    expect(screen.queryByPlaceholderText("Search saved filters")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Target.*4000 available/ }));
    expect(screen.getAllByRole("button", { name: /^Host / })).toHaveLength(50);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.queryByRole("button", { name: "Host 0000" })).not.toBeInTheDocument();
    const option = screen.getByRole("button", { name: "Host 0050" });
    option.focus();
    fireEvent.keyDown(option, { key: " " });
    fireEvent.click(option);
    expect(option).toHaveFocus();
    expect(option).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(screen.getByPlaceholderText("Search Target values"), { target: { value: "not present" } });
    expect(screen.getByText("No results found")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("Search Target values"), { target: { value: "3999" } });
    expect(screen.getByRole("button", { name: "Host 3999" })).toBeInTheDocument();
  });

  it("bounds selected chips and finds selected values absent from hosts", () => {
    const selected = emptyFilterState();
    selected.site = new Set(Array.from({ length: 120 }, (_, index) => `Missing ${index}`));
    const onChange = vi.fn();
    renderFilters({ selected, onChange });
    expect(screen.getAllByRole("button", { name: /^Remove Site:/ })).toHaveLength(3);
    expect(screen.getByText("+117 more selected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Review Site/ }));
    expect(screen.getByRole("button", { name: "Selected only" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(screen.getByPlaceholderText("Search Site values"), { target: { value: "Missing 119" } });
    fireEvent.click(screen.getByRole("button", { name: "Missing 119" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onChange).toHaveBeenCalledTimes(1);
    const result = onChange.mock.calls[0]![0];
    expect(result.site.size).toBe(119);
    expect(result.site.has("Missing 119")).toBe(false);
    expect(selected.site.size).toBe(120);
  });

  it("can leave selected-only mode after removing the last selection", () => {
    const selected = emptyFilterState();
    selected.site.add("Frankfurt");
    renderFilters({ selected });
    fireEvent.click(screen.getByRole("button", { name: /Review Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Selected only" }));
    expect(screen.getByRole("button", { name: "Frankfurt" })).toHaveAttribute("aria-pressed", "false");
  });

  it("moves focus to the next selected value when removing a focused selected-only option", () => {
    const selected = emptyFilterState();
    selected.site = new Set(["Missing A", "Missing B"]);
    renderFilters({ selected });
    fireEvent.click(screen.getByRole("button", { name: /Review Site/ }));
    const first = screen.getByRole("button", { name: "Missing A" });
    first.focus();
    fireEvent.keyDown(first, { key: " " });
    fireEvent.click(first);
    expect(screen.getByRole("button", { name: "Missing B" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Missing B" }));
    expect(screen.getByRole("button", { name: /Site 1 available/ })).toHaveFocus();
  });
});

describe("saved filter feedback", () => {
  it("shows loading, empty state and private/public cues", () => {
    const preset = { id: "public", name: "Team", visibility: "public" as const, filters: { status: ["active"] }, createdAt: "", updatedAt: "" };
    const { unmount } = renderFilters({ privatePresets: [], presetsLoading: true });
    expect(screen.getByRole("status")).toHaveTextContent("Loading saved filters");
    expect(screen.queryByPlaceholderText("Search saved filters")).not.toBeInTheDocument();
    unmount();
    const view = renderFilters({ privatePresets: [] });
    expect(screen.getByText("No saved filters")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Search saved filters")).not.toBeInTheDocument();
    view.unmount();
    renderFilters({ publicPresets: [preset] });
    fireEvent.click(screen.getByRole("button", { name: /Saved filters 2/ }));
    expect(screen.getByText("Private")).toBeInTheDocument();
    expect(screen.getByText("Public")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete saved filter Team" })).toBeInTheDocument();
  });

  it("keeps the draft on save failure and reports delete failure", async () => {
    const onSavePreset = vi.fn().mockRejectedValue(new Error("save"));
    const onDeletePreset = vi.fn().mockRejectedValue(new Error("delete"));
    const { onChange } = renderFilters({ onSavePreset, onDeletePreset });
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Save filter" }));
    const dialog = screen.getByRole("dialog", { name: "Save filter" });
    fireEvent.change(within(dialog).getByPlaceholderText("Filter name"), { target: { value: "Draft" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save filter" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not save filter");
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Save filter" })).not.toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: /Saved filters 1/ }));
    fireEvent.click(screen.getByRole("button", { name: "Delete saved filter Active" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not delete filter");
    expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled();
  });
});
