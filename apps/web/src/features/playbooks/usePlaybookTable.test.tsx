import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { usePlaybookTable } from "./usePlaybookTable";
import { PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG } from "./tableConfig";

describe("playbook child table URL", () => {
  it("persists filter, sorting and pagination updates, restores Back, and cannot override parent scope", async () => {
    const list = vi.fn().mockResolvedValue({ items: [], total: 500 });
    function Table() {
      const table = usePlaybookTable({ listFn: list, queryKey: ["assignments"], ...PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG, pageSizeDefault: 100 }, "parent-id", "assignments");
      const location = useLocation();
      const navigate = useNavigate();
      return <>
        <output>{location.search}</output>
        <button onClick={() => table.setColumnFilter("enabled", ["false"])}>Filter</button>
        <button onClick={() => table.setColumnFilter("playbook", ["foreign-id"])}>Override</button>
        <button onClick={() => table.setSortState({ key: "priority", dir: "desc" })}>Sort</button>
        <button onClick={() => table.setPageSize(25)}>Size</button>
        <button onClick={() => table.setPage(3)}>Page</button>
        <button onClick={() => void navigate(-1)}>Back</button>
      </>;
    }
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter initialEntries={["/playbooks/id?section=assignments&assignments.page=-3&assignments.size=Infinity&assignments.sort=invalid&assignments.playbook=foreign"]}><Table /></MemoryRouter></QueryClientProvider>);
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ playbook_id: "parent-id", limit: 100, offset: 0, sort: undefined })));
    fireEvent.click(screen.getByText("Override"));
    expect(list).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByText("Filter"));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ enabled: "false", playbook_id: "parent-id" })));
    fireEvent.click(screen.getByText("Sort"));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "priority", order: "desc" })));
    fireEvent.click(screen.getByText("Size"));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 25, offset: 0 })));
    fireEvent.click(screen.getByText("Page"));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 25, offset: 75 })));
    expect(screen.getByRole("status").textContent).toContain("section=assignments");
    fireEvent.click(screen.getByText("Back"));
    await waitFor(() => expect(screen.getByRole("status").textContent).not.toContain("assignments.page=3"));
    expect(screen.getByRole("status").textContent).toContain("assignments.enabled=false");
  });
});
