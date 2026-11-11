import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { ApiError } from "@/shared/api/client";
import { DeleteTargetSetDialog } from "./DeleteTargetSetDialog";

const usage = {
  job_templates: [{ id: "job-1", name: "Deploy job" }],
  assignments: [{ id: "assignment-1", name: null }],
  schedules: [{ id: "schedule-1", name: "Nightly" }],
};

function renderDialog(props: Partial<Parameters<typeof DeleteTargetSetDialog>[0]> = {}) {
  const onOpenChange = vi.fn();
  const onConfirm = vi.fn();
  render(
    <MemoryRouter>
      <DeleteTargetSetDialog
        open
        name="SSH targets"
        pending={false}
        onOpenChange={onOpenChange}
        onConfirm={onConfirm}
        {...props}
      />
    </MemoryRouter>,
  );
  return { onOpenChange, onConfirm };
}

describe("DeleteTargetSetDialog", () => {
  it("lists preflight usage with reassign links and blocks deletion", () => {
    renderDialog({ usage });
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Target set is in use")).toBeInTheDocument();
    expect(within(dialog).getByText("Deploy job")).toBeInTheDocument();
    expect(within(dialog).getByText("Nightly")).toBeInTheDocument();
    expect(within(dialog).getByText("assignment-1")).toBeInTheDocument();
    expect(within(dialog).getAllByRole("link", { name: /Reassign/ })).toHaveLength(3);
    expect(within(dialog).queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("renders structured 409 blockers when the delete is refused", () => {
    const error = new ApiError({
      status: 409,
      code: "DELETE_BLOCKED",
      reasonCode: "DELETE_BLOCKED",
      message: "Cannot delete this record while related objects still exist",
      details: {
        total_count: 2,
        blockers: [
          {
            table: "playbook_job_template",
            count: 2,
            examples: [{ id: "job-1", label: "Deploy job" }],
          },
        ],
      },
    });
    renderDialog({ usage, error });
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Deploy job")).toBeInTheDocument();
    expect(within(dialog).getByText("and 1 more")).toBeInTheDocument();
    expect(within(dialog).getAllByRole("link", { name: /Reassign/ })).toHaveLength(1);
    expect(within(dialog).queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("allows deleting a free set and cancel does not confirm", () => {
    const { onOpenChange, onConfirm } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
