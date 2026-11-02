import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import MultiSelect from "./MultiSelect";

const id = "00000000-0000-4000-8000-000000009401";
const unavailable = "00000000-0000-4000-8000-000000009402";
beforeEach(async () => { await i18n.changeLanguage("en"); });

describe("shared catalog membership selection", () => {
  it("retains and allows removal of selected IDs absent from the current catalog page", () => {
    const change = vi.fn();
    render(<MultiSelect aria-label="Tags" items={[{ id, label: "Core" }]} value={[id, unavailable]} onValueChange={change} />);
    expect(screen.getByText(unavailable)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Remove Core" }));
    expect(change).toHaveBeenLastCalledWith([unavailable]);
    fireEvent.click(screen.getByRole("button", { name: `Remove ${unavailable}` }));
    expect(change).toHaveBeenLastCalledWith([id]);
  });

  it("updates authorized labels/color without changing selected identity and rejects arbitrary CSS colors", () => {
    const change = vi.fn();
    const view = render(<MultiSelect items={[{ id, label: "Core", color: "155dfc" }]} value={[id]} onValueChange={change} />);
    expect(screen.getByText("Core").parentElement?.querySelector('[aria-hidden="true"]')).toHaveStyle({ backgroundColor: "#155dfc" });
    view.rerender(<MultiSelect items={[{ id, label: "Renamed", color: "url(https://invalid.test)" }]} value={[id]} onValueChange={change} />);
    expect(screen.queryByText("Core")).not.toBeInTheDocument();
    expect(screen.getByText("Renamed").parentElement?.querySelector('[aria-hidden="true"]')).toBeNull();
    expect(change).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove Renamed" }));
    expect(change).toHaveBeenCalledWith([]);
  });

  it("keeps disabled memberships visible and prevents editing", () => {
    render(<MultiSelect aria-label="Tags" items={[]} value={[unavailable]} onValueChange={vi.fn()} disabled />);
    expect(screen.getByText(unavailable)).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Tags" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Remove/ })).not.toBeInTheDocument();
  });
});
