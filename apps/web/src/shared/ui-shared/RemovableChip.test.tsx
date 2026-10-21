import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import RemovableChip from "@/shared/ui-shared/RemovableChip";

describe("RemovableChip", () => {
  it("renders the label and a dimmed sublabel prefix", () => {
    render(<RemovableChip label="admin" sublabel="role" />);
    expect(screen.getByText("admin")).toBeInTheDocument();
    expect(screen.getByText("role:")).toBeInTheDocument();
  });

  it("calls onRemove from the trailing button", () => {
    const onRemove = vi.fn();
    render(<RemovableChip label="READ" onRemove={onRemove} removeLabel="Remove READ" />);
    fireEvent.click(screen.getByRole("button", { name: "Remove READ" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("omits the remove button when onRemove is absent", () => {
    render(<RemovableChip label="READ" />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});
