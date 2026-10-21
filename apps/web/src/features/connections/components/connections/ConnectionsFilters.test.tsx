import { fireEvent, render, screen, within, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { testUuid } from "@/test/ids";
import { Dialog, DialogContent } from "@/shared/ui-primitives/dialog";
import ConnectionsFilters from "./ConnectionsFilters";
import { emptyFilterState } from "./filters-utils";

function renderFilters() {
  const onClose = vi.fn(), onChange = vi.fn(), onApplyPreset = vi.fn();
  const onSavePreset = vi.fn();
  render(<Dialog open><DialogContent><ConnectionsFilters hosts={[{ id: testUuid("target"), name: "Host", site: "Frankfurt", targetKind: "device", status: "active" }]}
    selected={emptyFilterState()} onClose={onClose} onChange={onChange} privatePresets={[{ id: "preset", name: "Active", visibility: "private", filters: { status: ["active"] }, createdAt: "", updatedAt: "" }]}
    publicPresets={[]} onApplyPreset={onApplyPreset} onSavePreset={onSavePreset} onDeletePreset={vi.fn()} /></DialogContent></Dialog>);
  return { onClose, onChange, onApplyPreset, onSavePreset };
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
    fireEvent.click(screen.getByRole("button", { name: /Active.*filter/ }));
    fireEvent.click(screen.getByRole("button", { name: /Site/ }));
    fireEvent.click(screen.getByRole("button", { name: "Frankfurt" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ site: new Set(["Frankfurt"]), status: new Set(["active"]) }));
    expect(onApplyPreset).not.toHaveBeenCalled();
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
