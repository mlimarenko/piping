import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import type { PlaybookTargetSetListItem } from "./api";
import { TargetSetsPanel } from "./TargetSetsPanel";

const tableProps = {
  loading: false,
  error: null,
  onRetry: vi.fn(),
  selectedId: null,
  sortState: null,
  onSortChange: vi.fn(),
  columnFilters: {},
  onColumnFilterChange: vi.fn(),
  onEdit: vi.fn(),
  onDelete: vi.fn(),
  onDuplicate: vi.fn(),
  onSelect: vi.fn(),
};

function item(overrides: Partial<PlaybookTargetSetListItem>): PlaybookTargetSetListItem {
  return {
    id: "targets-1",
    name: "Targets",
    mode: "dynamic",
    entity_kind: "connection_target",
    selector: {},
    created_at: "2026-06-06T00:00:00Z",
    updated_at: "2026-06-06T00:00:00Z",
    resolved_count: 0,
    usage: { job_templates: [], assignments: [], schedules: [] },
    created_by_username: "admin",
    updated_by_username: null,
    ...overrides,
  };
}

function renderPanel(targetSets: PlaybookTargetSetListItem[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AllPermissions>
          <TargetSetsPanel {...tableProps} targetSets={targetSets} />
        </AllPermissions>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("target set list row", () => {
  it("distinguishes zero, one, a thousand and an unknown host count", () => {
    renderPanel([
      item({ id: "zero", name: "Empty set", resolved_count: 0 }),
      item({ id: "one", name: "Single host", resolved_count: 1 }),
      item({ id: "many", name: "Large set", resolved_count: 1000 }),
      item({ id: "unknown", name: "Broken set", resolved_count: null }),
    ]);
    const rowText = (name: string) => screen.getByText(name).closest("tr")?.textContent ?? "";
    expect(rowText("Empty set")).toContain("0");
    expect(rowText("Single host")).toContain("1");
    expect(rowText("Large set")).toContain((1000).toLocaleString());
    expect(rowText("Broken set")).toContain("unknown");
  });

  it("links usage to the existing editors and shows the owner once", () => {
    renderPanel([
      item({
        id: "used",
        name: "Used set",
        updated_by_username: "operator",
        usage: {
          job_templates: [{ id: "job-1", name: "Deploy job" }],
          assignments: [{ id: "assignment-1", name: "Deploy assignment" }],
          schedules: [{ id: "schedule-1", name: "Nightly" }],
        },
      }),
    ]);
    expect(screen.getByRole("link", { name: "Deploy job" })).toHaveAttribute(
      "href",
      "/playbooks/jobs/job-1/edit",
    );
    expect(screen.getByRole("link", { name: "Deploy assignment" })).toHaveAttribute(
      "href",
      "/playbooks/settings/assignments",
    );
    expect(screen.getByRole("link", { name: "Nightly" })).toHaveAttribute(
      "href",
      "/playbooks/schedules/schedule-1/edit",
    );
    expect(screen.getByText("operator")).toBeInTheDocument();
  });
});
