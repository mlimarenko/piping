import { createRef } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Input } from "@/shared/ui-primitives/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/shared/ui-primitives/select";
import FormField, { FormFieldControl } from "./FormField";

describe("FormField control association", () => {
  it("merges existing descriptions without duplicates and replaces help with error without stale ids", () => {
    const field = (help?: string, error?: string) => (
      <>
        <p id="external">External description</p>
        <FormField label="Name" htmlFor="name" help={help} error={error}>
          <FormFieldControl aria-describedby="external external">
            <Input id="name" aria-describedby="external" aria-invalid={undefined} />
          </FormFieldControl>
        </FormField>
      </>
    );
    const { rerender } = render(field("Helpful"));
    const input = screen.getByRole("textbox", { name: "Name" });
    expect(input).toHaveAccessibleDescription("External description Helpful");
    const helpId = screen.getByText("Helpful").id;
    expect(input.getAttribute("aria-describedby")?.split(" ")).toEqual(["external", helpId]);
    rerender(field("Helpful", "Invalid name"));
    expect(input).toHaveAccessibleDescription("External description Invalid name");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(document.getElementById(helpId)).toBeNull();
    expect(input.getAttribute("aria-describedby")).not.toContain(helpId);
    const errorId = screen.getByRole("alert").id;
    rerender(field("Helpful"));
    expect(screen.getByText("Helpful")).toHaveAttribute("id", helpId);
    expect(input).toHaveAccessibleDescription("External description Helpful");
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(document.getElementById(errorId)).toBeNull();
    rerender(field());
    expect(input).toHaveAccessibleDescription("External description");
    expect(input).toHaveAttribute("aria-describedby", "external");
  });

  it("preserves explicit invalid state, control and slot refs, events and focus", () => {
    const inputRef = createRef<HTMLInputElement>();
    const slotRef = createRef<HTMLElement>();
    const onChange = vi.fn();
    const onChildFocus = vi.fn();
    const onSlotFocus = vi.fn();
    render(
      <FormField label="Value" htmlFor="value" error="Group warning">
        <FormFieldControl ref={slotRef} onFocus={onSlotFocus}>
          <Input id="value" ref={inputRef} aria-invalid={false} onFocus={onChildFocus} onChange={onChange} />
        </FormFieldControl>
      </FormField>,
    );
    const input = screen.getByLabelText("Value");
    expect(input).toHaveAttribute("id", "value");
    expect(input).toHaveAttribute("aria-invalid", "false");
    expect(inputRef.current).toBe(input);
    expect(slotRef.current).toBe(input);
    inputRef.current?.focus();
    expect(input).toHaveFocus();
    expect(onChildFocus).toHaveBeenCalledOnce();
    expect(onSlotFocus).toHaveBeenCalledOnce();
    fireEvent.change(input, { target: { value: "edited" } });
    expect(onChange).toHaveBeenCalledOnce();
  });

  it("associates a nested custom select trigger rather than the preceding action button", () => {
    render(
      <FormField label="Status" htmlFor="status" help="Select the status">
        <button type="button">Help action</button>
        <div>
          <Select defaultValue="active">
            <FormFieldControl>
              <SelectTrigger id="status"><SelectValue /></SelectTrigger>
            </FormFieldControl>
            <SelectContent><SelectItem value="active">Active</SelectItem></SelectContent>
          </Select>
        </div>
      </FormField>,
    );
    expect(screen.getByRole("combobox", { name: "Status" })).toHaveAccessibleDescription("Select the status");
    expect(screen.getByRole("button", { name: "Help action" })).not.toHaveAttribute("aria-describedby");
  });

  it("describes an explicitly labelled group without marking its action as the control", () => {
    render(
      <FormField error="Choose at least one">
        <FormFieldControl>
          <div role="group" aria-label="Choices"><input aria-label="Choice" /><button>Add</button></div>
        </FormFieldControl>
      </FormField>,
    );
    expect(screen.getByRole("group", { name: "Choices" })).toHaveAccessibleDescription("Choose at least one");
    expect(screen.getByRole("button", { name: "Add" })).not.toHaveAttribute("aria-invalid");
  });
});
