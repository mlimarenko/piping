import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TablePagination } from "./Pagination";

describe("TablePagination", () => {
  it("retains range, navigation and page size in a compact list", () => {
    const onPageChange = vi.fn();
    const onPageSizeChange = vi.fn();
    render(<TablePagination compact page={1} pageSize={25} total={500} pageSizeOptions={[25, 50]} onPageChange={onPageChange} onPageSizeChange={onPageSizeChange} />);

    expect(screen.getByText("26–50 of 500")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Go to page/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Previous page" }));
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(onPageChange.mock.calls).toEqual([[0], [2]]);
    fireEvent.change(screen.getByRole("combobox", { name: "Rows per page" }), { target: { value: "50" } });
    expect(onPageSizeChange).toHaveBeenCalledWith(50);
  });

  it("keeps compact navigation bounded for empty and final pages", () => {
    const props = { compact: true, pageSize: 25, onPageChange: vi.fn(), onPageSizeChange: vi.fn() };
    const { rerender } = render(<TablePagination {...props} page={0} total={0} />);
    expect(screen.getByText("0–0 of 0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();

    rerender(<TablePagination {...props} page={2} total={61} />);
    expect(screen.getByText("51–61 of 61")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
  });

  it("preserves direct numbered-page navigation for the full table footer", () => {
    const onPageChange = vi.fn();
    render(<TablePagination page={0} pageSize={25} total={100} onPageChange={onPageChange} onPageSizeChange={vi.fn()} />);

    expect(screen.getByRole("button", { name: "Go to page 1" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "Go to page 4" }));
    expect(onPageChange).toHaveBeenCalledWith(3);
  });
});
