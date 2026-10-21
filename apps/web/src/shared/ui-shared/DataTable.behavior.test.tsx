import { fireEvent, render, screen, within } from "@testing-library/react";
import "@/shared/i18n";

import DataTable from "./DataTable";
import type { UseTableColumnVisibilityResult } from "./useTableColumnVisibility";

const columnVisibilityState: UseTableColumnVisibilityResult = {
  hiddenColumns: new Set<string>(),
  setHidden: vi.fn(),
  reset: vi.fn(),
  source: "default",
};

vi.mock("./useTableColumnVisibility", () => ({
  useTableColumnVisibility: () => columnVisibilityState,
}));

interface Row {
  id: string;
  name: string;
}

const columns = [{ key: "name", label: "Name" }];
const emptyRows: Row[] = [];

beforeEach(() => {
  columnVisibilityState.hiddenColumns = new Set<string>();
  columnVisibilityState.setHidden = vi.fn();
  columnVisibilityState.reset = vi.fn();
  columnVisibilityState.source = "default";
});

/** Reads the trimmed text of each body row, skipping the header row. */
function bodyRowTexts(): string[] {
  return screen
    .getAllByRole("row")
    .slice(1)
    .map((row) => row.textContent?.trim() ?? "");
}

describe("DataTable uncontrolled client-side sorting", () => {
  const sortRows: Row[] = [
    { id: "b", name: "Banana" },
    { id: "a", name: "apple" },
    { id: "c", name: "Cherry" },
  ];

  it("reports mounted rows in sort order and updates on data replacement", () => {
    const report = vi.fn();
    const { rerender } = render(
      <DataTable columns={columns} data={sortRows} getRowKey={(row) => row.id} onRenderedRowsChange={report} />,
    );
    expect(report).toHaveBeenLastCalledWith(sortRows);
    fireEvent.click(screen.getByRole("button", { name: /name/i }));
    expect(report).toHaveBeenLastCalledWith([sortRows[1], sortRows[0], sortRows[2]]);
    rerender(<DataTable columns={columns} data={emptyRows} getRowKey={(row) => row.id} onRenderedRowsChange={report} />);
    expect(report).toHaveBeenLastCalledWith([]);
  });

  it("reorders rows through asc, desc, then back to the original order", () => {
    render(
      <DataTable columns={columns} data={sortRows} getRowKey={(row) => row.id} />,
    );

    // No sort applied yet: rows keep the caller-provided order.
    expect(bodyRowTexts()).toEqual(["Banana", "apple", "Cherry"]);

    const sortButton = screen.getByRole("button", { name: /name/i });

    // First click -> ascending (case-insensitive).
    fireEvent.click(sortButton);
    expect(bodyRowTexts()).toEqual(["apple", "Banana", "Cherry"]);

    // Second click -> descending.
    fireEvent.click(sortButton);
    expect(bodyRowTexts()).toEqual(["Cherry", "Banana", "apple"]);

    // Third click -> cleared, back to the original data order.
    fireEvent.click(sortButton);
    expect(bodyRowTexts()).toEqual(["Banana", "apple", "Cherry"]);
  });

  it("sorts by the column's getSortValue instead of the displayed field", () => {
    interface RankedRow {
      id: string;
      item: string;
      rank: number;
    }
    const rankedRows: RankedRow[] = [
      { id: "1", item: "Apple", rank: 2 },
      { id: "2", item: "Zebra", rank: 1 },
    ];

    render(
      <DataTable
        columns={[
          {
            key: "item",
            label: "Item",
            getSortValue: (row: RankedRow) => row.rank,
          },
        ]}
        data={rankedRows}
        getRowKey={(row) => row.id}
      />,
    );

    // Alphabetical order of the displayed field would be Apple, Zebra.
    fireEvent.click(screen.getByRole("button", { name: /item/i }));

    // getSortValue (rank) orders Zebra (1) ahead of Apple (2) instead.
    expect(bodyRowTexts()).toEqual(["Zebra", "Apple"]);
  });
});

describe("DataTable error state", () => {
  const rows: Row[] = [{ id: "row-1", name: "Persistent row" }];

  it("surfaces an Error instance message and triggers retry", () => {
    const onRetry = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={emptyRows}
        getRowKey={(row) => row.id}
        error={new Error("Upstream exploded")}
        onRetry={onRetry}
      />,
    );

    const alert = screen.getByRole("alert");
    expect(within(alert).getByText("Failed to load data")).toBeInTheDocument();
    expect(within(alert).getByText("Upstream exploded")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("renders a plain string error and omits retry when no handler is given", () => {
    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        error="Raw string failure"
      />,
    );

    const alert = screen.getByRole("alert");
    expect(within(alert).getByText("Raw string failure")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /retry/i })).not.toBeInTheDocument();
  });

  it("reads the message from an error-like object", () => {
    render(
      <DataTable
        columns={columns}
        data={emptyRows}
        getRowKey={(row) => row.id}
        error={{ message: "Object-shaped failure" }}
      />,
    );

    expect(
      within(screen.getByRole("alert")).getByText("Object-shaped failure"),
    ).toBeInTheDocument();
  });

  it("falls back to the default description for an unrecognized error", () => {
    render(
      <DataTable
        columns={columns}
        data={emptyRows}
        getRowKey={(row) => row.id}
        error={{ code: "NO_MESSAGE" }}
      />,
    );

    const alert = screen.getByRole("alert");
    expect(within(alert).getByText("Failed to load data")).toBeInTheDocument();
    expect(
      within(alert).getByText("Something went wrong while loading this list."),
    ).toBeInTheDocument();
  });
});

describe("DataTable empty and loading states", () => {
  it("renders the provided empty state when there is no data", () => {
    render(
      <DataTable
        columns={columns}
        data={emptyRows}
        getRowKey={(row) => row.id}
        emptyState={<div>Nothing to show here</div>}
      />,
    );

    expect(screen.getByText("Nothing to show here")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows five skeleton placeholder rows during the initial load", () => {
    render(
      <DataTable columns={columns} data={emptyRows} getRowKey={(row) => row.id} loading />,
    );

    // Header row + 5 skeleton rows, and no error/empty/data content.
    expect(screen.getAllByRole("row")).toHaveLength(6);
    expect(screen.getByRole("columnheader", { name: /name/i })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

describe("DataTable toolbar and footer rows", () => {
  const multiColumns = [
    { key: "name", label: "Name" },
    { key: "status", label: "Status" },
  ];
  const multiRows = [{ id: "row-1", name: "Alpha", status: "Active" }];

  it("places the column-settings control in the toolbar row, not the header", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="toolbar-table"
        toolbar={<span>Custom toolbar</span>}
      />,
    );

    expect(screen.getByText("Custom toolbar")).toBeInTheDocument();

    const columnsButton = screen.getByRole("button", { name: /columns/i });
    // When a toolbar is present the settings button lives in the toolbar row,
    // so it must not be nested inside a table header cell.
    expect(columnsButton.closest("th")).toBeNull();
    expect(columnsButton.closest("thead")).toBeNull();
  });

  it("renders a custom footer when pagination is not configured", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        footer={<span>Footer summary</span>}
      />,
    );

    expect(screen.getByText("Footer summary")).toBeInTheDocument();
  });
});

describe("DataTable cell rendering", () => {
  it("stringifies numeric and boolean cell values when no render fn is given", () => {
    interface MetricRow {
      id: string;
      count: number;
      active: boolean;
    }
    const metricRows: MetricRow[] = [{ id: "m-1", count: 42, active: true }];

    render(
      <DataTable
        columns={[
          { key: "count", label: "Count" },
          { key: "active", label: "Active" },
        ]}
        data={metricRows}
        getRowKey={(row) => row.id}
      />,
    );

    expect(screen.getByRole("cell", { name: "42" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "true" })).toBeInTheDocument();
  });
});

describe("DataTable row selection and interaction", () => {
  const selectRows = [
    { id: "row-1", name: "Alpha" },
    { id: "row-2", name: "Bravo" },
  ];

  it("reflects and toggles per-row selection without firing a row click", () => {
    const onSelectionChange = vi.fn();
    const onRowClick = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={selectRows}
        getRowKey={(row) => row.id}
        selectable
        selectedKeys={new Set(["row-2"])}
        onSelectionChange={onSelectionChange}
        onRowClick={onRowClick}
      />,
    );

    const firstRowCheckbox = screen.getByRole("checkbox", { name: /select row row-1/i });
    const secondRowCheckbox = screen.getByRole("checkbox", { name: /select row row-2/i });
    expect(firstRowCheckbox).not.toBeChecked();
    expect(secondRowCheckbox).toBeChecked();

    // Selecting an unselected row adds it to the set.
    fireEvent.click(firstRowCheckbox);
    expect(onSelectionChange).toHaveBeenCalledWith(new Set(["row-1", "row-2"]));
    // The checkbox click must not bubble into a row-level click.
    expect(onRowClick).not.toHaveBeenCalled();

    // Toggling an already-selected row removes it.
    fireEvent.click(secondRowCheckbox);
    expect(onSelectionChange).toHaveBeenLastCalledWith(new Set<string>());
  });

  it("activates a clickable row via Enter and Space but ignores other keys", () => {
    const onRowClick = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={selectRows.slice(0, 1)}
        getRowKey={(row) => row.id}
        onRowClick={onRowClick}
      />,
    );

    const row = screen.getByRole("button", { name: /alpha/i });

    fireEvent.keyDown(row, { key: "a" });
    expect(onRowClick).not.toHaveBeenCalled();

    fireEvent.keyDown(row, { key: "Enter" });
    fireEvent.keyDown(row, { key: " " });

    expect(onRowClick).toHaveBeenCalledTimes(2);
    expect(onRowClick).toHaveBeenNthCalledWith(1, selectRows[0], row);
    expect(onRowClick).toHaveBeenNthCalledWith(2, selectRows[0], row);
  });

  it("marks the currently selected row and leaves the others unstyled", () => {
    render(
      <DataTable
        columns={columns}
        data={selectRows}
        getRowKey={(row) => row.id}
        selectedRow={selectRows[0]}
      />,
    );

    const selectedTr = screen.getByText("Alpha").closest("tr");
    const otherTr = screen.getByText("Bravo").closest("tr");

    expect(selectedTr?.className).toContain("bg-primary/6");
    expect(otherTr?.className).not.toContain("bg-primary/6");
    expect(otherTr?.className).toContain("hover:bg-accent/20");
  });
});

describe("DataTable search-empty states", () => {
  const multiColumns = [
    { key: "name", label: "Name" },
    { key: "status", label: "Status" },
    { key: "owner", label: "Owner" },
  ];
  const multiRows = [{ id: "row-1", name: "Alpha", status: "Active", owner: "Sam" }];

  it("filters the column-settings list and shows an empty note for no matches", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="search-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const search = within(dialog).getByPlaceholderText(/search values/i);

    fireEvent.change(search, { target: { value: "own" } });
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /owner/i })).toBeInTheDocument();
    expect(
      within(dialog).queryByRole("menuitemcheckbox", { name: /status/i }),
    ).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: "zzz-no-match" } });
    expect(within(dialog).getByText(/no matching values/i)).toBeInTheDocument();
  });

  it("reveals a previously hidden column when its settings toggle is clicked", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="reveal-table"
      />,
    );

    // "status" starts hidden.
    expect(screen.queryByRole("columnheader", { name: /status/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const statusToggle = within(dialog).getByRole("menuitemcheckbox", { name: /status/i });
    expect(statusToggle).toHaveAttribute("aria-checked", "false");

    // Clicking an unchecked, enabled column removes it from the hidden set.
    fireEvent.click(statusToggle);
    expect(columnVisibilityState.setHidden).toHaveBeenCalledWith(new Set<string>());
  });

  it("shows an empty note when a column filter search matches nothing", () => {
    render(
      <DataTable
        columns={[
          {
            key: "status",
            label: "Status",
            filterOptions: [
              { value: "active", label: "Active", count: 1 },
              { value: "paused", label: "Paused", count: 2 },
            ],
          },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        columnFilters={{ status: [] }}
        onColumnFilterChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /filter.*status/i }));
    const dialog = screen.getByRole("dialog");
    const search = within(dialog).getByPlaceholderText(/search values/i);

    fireEvent.change(search, { target: { value: "no-such-value" } });
    expect(within(dialog).getByText(/no matching values/i)).toBeInTheDocument();
  });
});
