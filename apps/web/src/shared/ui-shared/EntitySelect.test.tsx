import { createRef } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField, { FormFieldControl } from "./FormField";

const options = [
  { id: "role-admin", label: "Administrators", searchText: "full access root" },
  { id: "role-viewer", label: "Viewers", searchText: "read only audience" },
  { id: "role-audit", label: "Audit team", searchText: "logs compliance" },
];

describe("EntitySelect", () => {
  it("forwards field descriptions, explicit invalid state and refs to its actual trigger", () => {
    const ref = createRef<HTMLButtonElement>();
    const slotRef = createRef<HTMLElement>();
    const onFocus = vi.fn();
    const field = (error?: string) => <>
      <p id="role-note">External role note</p>
      <FormField label="Role" htmlFor="role-picker" help="Choose a role" error={error}>
        <FormFieldControl ref={slotRef}>
          <EntitySelect ref={ref} id="role-picker" items={options} value="" onValueChange={vi.fn()}
            aria-label="Role" aria-describedby="role-note" aria-invalid={false} onFocus={onFocus} />
        </FormFieldControl>
      </FormField>
    </>;
    const { rerender } = render(field("Role is required"));
    const trigger = screen.getByRole("combobox", { name: "Role" });
    expect(trigger).toHaveAccessibleDescription("External role note Role is required");
    expect(trigger).toHaveAttribute("aria-invalid", "false");
    expect(trigger).toHaveAttribute("id", "role-picker");
    expect(ref.current).toBe(trigger);
    expect(slotRef.current).toBe(trigger);
    ref.current?.focus();
    expect(trigger).toHaveFocus();
    expect(onFocus).toHaveBeenCalledOnce();
    const errorId = screen.getByRole("alert").id;
    rerender(field());
    expect(trigger).toHaveAccessibleDescription("External role note Choose a role");
    expect(trigger.getAttribute("aria-describedby")).not.toContain(errorId);
  });

  it("retains trigger handlers, keyboard selection and focus return through the picker chain", async () => {
    const ref = createRef<HTMLButtonElement>();
    const onClick = vi.fn();
    const onValueChange = vi.fn();
    render(<EntitySelect ref={ref} items={options} value="" onValueChange={onValueChange}
      onClick={onClick} aria-label="Role" searchPlaceholder="Search roles" />);
    const trigger = screen.getByRole("combobox", { name: "Role" });
    fireEvent.click(trigger);
    expect(onClick).toHaveBeenCalledOnce();
    const search = screen.getByPlaceholderText("Search roles");
    expect(search).toHaveFocus();
    fireEvent.keyDown(search, { key: "ArrowDown" });
    fireEvent.keyDown(search, { key: "Enter" });
    expect(onValueChange).toHaveBeenCalledWith("role-viewer");
    await waitFor(() => expect(ref.current).toHaveFocus());
    expect(trigger).toHaveAttribute("aria-expanded", "false");
  });

  it("renders a searchable list and submits option ids", () => {
    const onValueChange = vi.fn();
    render(
      <EntitySelect
        items={options}
        value=""
        onValueChange={onValueChange}
        placeholder="Choose role"
        searchPlaceholder="Search roles"
        aria-label="Role"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Role" }));
    const listbox = screen.getByRole("listbox");
    expect(within(listbox).getByText("Administrators")).toBeInTheDocument();

    fireEvent.click(within(listbox).getByText("Administrators"));

    expect(onValueChange).toHaveBeenCalledWith("role-admin");
  });

  it("searches labels, ids and extra searchText consistently", () => {
    render(
      <EntitySelect
        items={options}
        value=""
        onValueChange={() => {}}
        placeholder="Choose role"
        searchPlaceholder="Search roles"
        aria-label="Role"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Role" }));
    fireEvent.change(screen.getByPlaceholderText("Search roles"), {
      target: { value: "compliance" },
    });

    const listbox = screen.getByRole("listbox");
    expect(within(listbox).getByText("Audit team")).toBeInTheDocument();
    expect(within(listbox).queryByText("Administrators")).not.toBeInTheDocument();
  });

  it("shows the selected label from the current option list", () => {
    render(
      <EntitySelect
        items={options}
        value="role-viewer"
        onValueChange={() => {}}
        placeholder="Choose role"
        aria-label="Role"
      />,
    );

    expect(screen.getByRole("combobox", { name: "Role" })).toHaveTextContent("Viewers");
  });
});
