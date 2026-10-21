import { createRef } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import RowActions from "./RowActions";

describe("RowActions", () => {
  it("opens by keyboard, skips disabled children, invokes an action and returns focus", async () => {
    const ref = createRef<HTMLButtonElement>();
    const edit = vi.fn();
    const denied = vi.fn();
    const remove = vi.fn();
    render(
      <RowActions ref={ref} label="Actions for router-01">
        <DropdownMenuItem onSelect={edit}>Edit</DropdownMenuItem>
        <DropdownMenuItem disabled onSelect={denied}>Restricted action</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" onSelect={remove}>Delete</DropdownMenuItem>
      </RowActions>,
    );
    const trigger = screen.getByRole("button", { name: "Actions for router-01" });
    expect(ref.current).toBe(trigger);
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const editItem = await screen.findByRole("menuitem", { name: "Edit" });
    await waitFor(() => expect(editItem).toHaveFocus());
    expect(screen.getByRole("menuitem", { name: "Restricted action" })).toHaveAttribute("aria-disabled", "true");
    fireEvent.keyDown(editItem, { key: "ArrowDown" });
    const deleteItem = screen.getByRole("menuitem", { name: "Delete" });
    await waitFor(() => expect(deleteItem).toHaveFocus());
    fireEvent.keyDown(deleteItem, { key: "Enter" });
    expect(remove).toHaveBeenCalledTimes(1);
    expect(edit).not.toHaveBeenCalled();
    expect(denied).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("closes on Escape without selecting and returns focus to the row trigger", async () => {
    const select = vi.fn();
    render(
      <RowActions label="Actions for router-02">
        <DropdownMenuItem onSelect={select}>View</DropdownMenuItem>
      </RowActions>,
    );
    const trigger = screen.getByRole("button", { name: "Actions for router-02" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    const item = await screen.findByRole("menuitem", { name: "View" });
    await waitFor(() => expect(item).toHaveFocus());
    fireEvent.keyDown(item, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(select).not.toHaveBeenCalled();
  });

  it("keeps authorization in supplied children and respects a disabled trigger", async () => {
    const { rerender } = render(
      <RowActions label="Actions for router-03" disabled>
        <DropdownMenuItem>View</DropdownMenuItem>
      </RowActions>,
    );
    const trigger = screen.getByRole("button", { name: "Actions for router-03" });
    expect(trigger).toBeDisabled();
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    rerender(
      <RowActions label="Actions for router-03">
        <DropdownMenuItem>View</DropdownMenuItem>
      </RowActions>,
    );
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    expect(await screen.findByRole("menuitem", { name: "View" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("does not activate the surrounding row or submit a form", async () => {
    const rowClick = vi.fn();
    const submit = vi.fn((event: React.FormEvent) => event.preventDefault());
    const select = vi.fn();
    render(
      <form onSubmit={submit}>
        <div onClick={rowClick}>
          <RowActions label="Actions for router-04">
            <DropdownMenuItem onSelect={select}>View</DropdownMenuItem>
          </RowActions>
        </div>
      </form>,
    );
    const trigger = screen.getByRole("button", { name: "Actions for router-04" });
    fireEvent.click(trigger);
    expect(rowClick).not.toHaveBeenCalled();
    expect(submit).not.toHaveBeenCalled();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "View" }));
    expect(select).toHaveBeenCalledTimes(1);
    expect(rowClick).not.toHaveBeenCalled();
    expect(submit).not.toHaveBeenCalled();
  });
});
