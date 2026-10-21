import { createRef, type MouseEvent } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import SearchableSelect from "@/shared/ui-shared/SearchableSelect";

const items = [
  { id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", label: "Alpha" },
  { id: "beta-id", label: "Beta" },
];

describe("SearchableSelect", () => {
  it("forwards standard control attributes and ref to the button, not the panel wrapper", () => {
    const ref = createRef<HTMLButtonElement>();
    const onFocus = vi.fn();
    render(<>
      <p id="pick-note">Picker description</p>
      <SearchableSelect ref={ref} id="pick" items={items} value="" onValueChange={vi.fn()}
        aria-label="Pick" aria-describedby="pick-note" aria-invalid="grammar" onFocus={onFocus} />
    </>);
    const trigger = screen.getByRole("combobox", { name: "Pick" });
    expect(trigger).toHaveAccessibleDescription("Picker description");
    expect(trigger).toHaveAttribute("aria-invalid", "grammar");
    expect(ref.current).toBe(trigger);
    ref.current?.focus();
    expect(trigger).toHaveFocus();
    expect(onFocus).toHaveBeenCalledOnce();
  });

  it("honors a trigger click handler that prevents opening", () => {
    const onClick = vi.fn((event: MouseEvent<HTMLButtonElement>) => event.preventDefault());
    render(<SearchableSelect items={items} value="" onValueChange={vi.fn()} aria-label="Pick" onClick={onClick} />);
    const trigger = screen.getByRole("combobox", { name: "Pick" });
    fireEvent.click(trigger);
    expect(onClick).toHaveBeenCalledOnce();
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("binds option ids by construction: onValueChange receives item.id, not the label", () => {
    const onValueChange = vi.fn();
    render(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={onValueChange}
        aria-label="Pick"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Pick" }));
    fireEvent.click(screen.getByText("Beta"));

    expect(onValueChange).toHaveBeenCalledWith("beta-id");
    expect(onValueChange).not.toHaveBeenCalledWith("Beta");
  });

  it("shows the matching item's label for the current value", () => {
    render(
      <SearchableSelect
        items={items}
        value="beta-id"
        onValueChange={() => {}}
        aria-label="Pick"
      />,
    );

    expect(screen.getByRole("combobox", { name: "Pick" })).toHaveTextContent("Beta");
  });

  it("falls back to selectedLabel when the value isn't in the current items page", () => {
    render(
      <SearchableSelect
        items={items}
        value="not-loaded-id"
        selectedLabel="Loaded elsewhere"
        onValueChange={() => {}}
        aria-label="Pick"
      />,
    );

    expect(screen.getByRole("combobox", { name: "Pick" })).toHaveTextContent("Loaded elsewhere");
  });

  it("truncates a bare UUID value with no label to xxxxxxxx...", () => {
    render(
      <SearchableSelect
        items={[]}
        value="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        onValueChange={() => {}}
        aria-label="Pick"
      />,
    );

    expect(screen.getByRole("combobox", { name: "Pick" })).toHaveTextContent("aaaaaaaa...");
  });

  it("supports controlled search with shouldFilter=false (host owns filtering)", () => {
    const onSearchValueChange = vi.fn();
    render(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={() => {}}
        shouldFilter={false}
        searchValue="que"
        onSearchValueChange={onSearchValueChange}
        aria-label="Pick"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Pick" }));
    const input = screen.getByPlaceholderText((text) => text.toLowerCase().includes("search"));
    expect(input).toHaveValue("que");

    fireEvent.change(input, { target: { value: "quer" } });
    expect(onSearchValueChange).toHaveBeenCalledWith("quer");
  });

  it("fires onOpenChange on open and close", () => {
    const onOpenChange = vi.fn();
    render(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={() => {}}
        onOpenChange={onOpenChange}
        aria-label="Pick"
      />,
    );

    const trigger = screen.getByRole("combobox", { name: "Pick" });
    fireEvent.click(trigger);
    expect(onOpenChange).toHaveBeenCalledWith(true);

    fireEvent.click(trigger);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("resets uncontrolled search text on close", () => {
    render(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={() => {}}
        aria-label="Pick"
      />,
    );

    const trigger = screen.getByRole("combobox", { name: "Pick" });
    fireEvent.click(trigger);
    const input = screen.getByPlaceholderText((text) => text.toLowerCase().includes("search"));
    fireEvent.change(input, { target: { value: "alp" } });
    expect(input).toHaveValue("alp");

    // Close then reopen: uncontrolled search should have reset.
    fireEvent.click(trigger);
    fireEvent.click(trigger);
    const reopenedInput = screen.getByPlaceholderText((text) => text.toLowerCase().includes("search"));
    expect(reopenedInput).toHaveValue("");
  });

  it("shows the loading state instead of the item list when isLoading", () => {
    render(
      <SearchableSelect
        items={items}
        value=""
        onValueChange={() => {}}
        isLoading
        loadingText="Loading options…"
        aria-label="Pick"
      />,
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Pick" }));
    expect(screen.getByText("Loading options…")).toBeInTheDocument();
    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
  });
});
