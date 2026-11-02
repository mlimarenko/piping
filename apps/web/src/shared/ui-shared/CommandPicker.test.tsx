import type { ComponentProps, ReactElement } from "react";
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "@/shared/i18n";

import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import { CommandPickerContent } from "./CommandPicker";

const items = [
  { id: "opt-1", label: "Option One" },
  { id: "opt-2", label: "Option Two" },
];

// A real, stateful Dialog (not a fixed `open` prop) so Radix can actually
// close it via onOpenChange when the user presses Escape a second time.
function DialogHost({
  extraProps = {},
}: {
  extraProps?: Partial<ComponentProps<typeof SearchableSelect>>;
}) {
  const [open, setOpen] = useState(true);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent description="Picker dialog">
        <DialogHeader>
          <DialogTitle>Pick an option</DialogTitle>
        </DialogHeader>
        <SearchableSelect
          items={items}
          value=""
          onValueChange={() => {}}
          aria-label="Pick option"
          {...extraProps}
        />
      </DialogContent>
    </Dialog>
  );
}

function renderInDialog(
  extraProps: Partial<ComponentProps<typeof SearchableSelect>> = {},
): ReturnType<typeof render> {
  return render(<DialogHost extraProps={extraProps} />);
}

function renderStandalone(element: ReactElement): ReturnType<typeof render> {
  return render(element);
}

describe("CommandPickerInline", () => {
  it("associates optional visible help with the search input without changing generic pickers", () => {
    const { rerender } = render(<CommandPickerContent searchPlaceholder="Search options" emptyText="No options" description="Choose an existing inventory object">{null}</CommandPickerContent>);
    const description = screen.getByText("Choose an existing inventory object");
    expect(screen.getByPlaceholderText("Search options")).toHaveAttribute("aria-describedby", description.id);
    rerender(<CommandPickerContent searchPlaceholder="Search options" emptyText="No options">{null}</CommandPickerContent>);
    expect(screen.queryByText("Choose an existing inventory object")).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText("Search options")).not.toHaveAttribute("aria-describedby");
  });

  it("renders the panel inline inside the modal dialog's DOM subtree, never portaled to body", () => {
    renderInDialog();

    fireEvent.click(screen.getByRole("combobox", { name: "Pick option" }));

    const panel = document.querySelector("[data-picker-panel]");
    const list = document.querySelector("[cmdk-list]");
    expect(panel).toBeInTheDocument();
    // The panel is a DOM descendant of the modal dialog (react-remove-scroll's
    // lock tree), the root cause this component exists to fix — a portaled
    // panel would sit outside this subtree and its wheel events would be
    // swallowed by the dialog's scroll lock.
    expect(panel?.closest('[role="dialog"]')).toBeInTheDocument();
    // Not independently portaled: it is never a direct child of <body>.
    expect(Array.from(document.body.children)).not.toContain(panel);
    expect(list).toHaveClass("overflow-y-auto", "overscroll-contain");
    expect(Array.from(list?.classList ?? [])).toEqual(
      expect.arrayContaining([expect.stringMatching(/^max-h-/)]),
    );
  });

  it("closes the panel on Escape without closing the dialog, then closes the dialog on the next Escape", () => {
    renderInDialog();

    fireEvent.click(screen.getByRole("combobox", { name: "Pick option" }));
    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();

    fireEvent.keyDown(document.body, { key: "Escape" });

    expect(document.querySelector("[data-picker-panel]")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    fireEvent.keyDown(document.body, { key: "Escape" });

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("closes when the user clicks outside the picker root, but not when clicking the trigger itself", () => {
    renderInDialog();

    const trigger = screen.getByRole("combobox", { name: "Pick option" });
    fireEvent.click(trigger);
    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();

    fireEvent.pointerDown(trigger);
    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();

    fireEvent.pointerDown(screen.getByRole("heading", { name: "Pick an option" }));
    expect(document.querySelector("[data-picker-panel]")).not.toBeInTheDocument();
  });

  it("overlay placement absolutely positions the panel and still never portals it", () => {
    renderStandalone(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={() => {}}
        aria-label="Pick option"
        panelPlacement="overlay"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Pick option" }));

    const root = document.querySelector("[data-picker-root]");
    const panel = document.querySelector("[data-picker-panel]");
    expect(root).toHaveClass("relative");
    expect(panel).toHaveClass("absolute", "z-50");
    expect(Array.from(document.body.children)).not.toContain(panel);
  });
});
