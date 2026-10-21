import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SearchInput from "@/shared/ui-shared/SearchInput";

describe("SearchInput", () => {
  it("emits the raw input value via onValueChange", () => {
    const onValueChange = vi.fn();
    render(<SearchInput value="" onValueChange={onValueChange} placeholder="Search" />);
    fireEvent.change(screen.getByPlaceholderText("Search"), { target: { value: "abc" } });
    expect(onValueChange).toHaveBeenCalledWith("abc");
  });

  it("shows a clear button only when onClear is set and the value is non-empty", () => {
    const onClear = vi.fn();
    const { rerender } = render(
      <SearchInput value="" onValueChange={() => {}} onClear={onClear} clearLabel="Clear" />,
    );
    expect(screen.queryByRole("button", { name: "Clear" })).toBeNull();

    rerender(
      <SearchInput value="x" onValueChange={() => {}} onClear={onClear} clearLabel="Clear" />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it("does not render a clear button without onClear", () => {
    render(<SearchInput value="x" onValueChange={() => {}} placeholder="Search" />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("keeps an accessible name when the placeholder is hidden by entered text", () => {
    const { rerender } = render(<SearchInput value="router" onValueChange={() => {}} placeholder="Search devices" />);
    expect(screen.getByRole("textbox", { name: "Search devices" })).toHaveValue("router");
    rerender(<SearchInput value="router" onValueChange={() => {}} placeholder="Search devices" aria-label="Filter inventory" />);
    expect(screen.getByRole("textbox", { name: "Filter inventory" })).toHaveValue("router");
  });
});
