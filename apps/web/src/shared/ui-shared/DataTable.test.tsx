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
const rows: Row[] = [{ id: "row-1", name: "Persistent row" }];

beforeEach(() => {
  columnVisibilityState.hiddenColumns = new Set<string>();
  columnVisibilityState.setHidden = vi.fn();
  columnVisibilityState.reset = vi.fn();
  columnVisibilityState.source = "default";
});

describe("DataTable", () => {
  it("keeps existing rows visible while a background load is in progress", () => {
    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        loading
      />,
    );

    expect(screen.getByText("Persistent row")).toBeInTheDocument();
  });

  it("keeps select-all scoped to the full current data set when rows are virtualized", () => {
    const manyRows = Array.from({ length: 400 }, (_, index) => ({
      id: `row-${index}`,
      name: `Row ${index}`,
    }));
    const onSelectionChange = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={manyRows}
        getRowKey={(row) => row.id}
        selectable
        selectedKeys={new Set()}
        onSelectionChange={onSelectionChange}
        virtualizeRows={{ threshold: 10 }}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: /select all/i }));

    expect(onSelectionChange).toHaveBeenCalledTimes(1);
    expect(onSelectionChange.mock.calls[0]?.[0]).toEqual(
      new Set(manyRows.map((row) => row.id)),
    );
  });

  it("reports row intent on hover and focus without requiring a row click", () => {
    const onRowClick = vi.fn();
    const onRowIntent = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        onRowClick={onRowClick}
        onRowIntent={onRowIntent}
      />,
    );

    const row = screen.getByRole("button", { name: /persistent row/i });
    fireEvent.pointerEnter(row);
    fireEvent.focus(row);

    expect(onRowIntent).toHaveBeenCalledTimes(2);
    expect(onRowIntent).toHaveBeenNthCalledWith(1, rows[0]);
    expect(onRowIntent).toHaveBeenNthCalledWith(2, rows[0]);
    expect(onRowClick).not.toHaveBeenCalled();
  });

  it("only exposes rows allowed by the row click predicate as actions", () => {
    const onRowClick = vi.fn();
    const mixedRows = [
      { id: "enabled", name: "Enabled row", enabled: true },
      { id: "disabled", name: "Disabled row", enabled: false },
    ];

    render(
      <DataTable
        columns={columns}
        data={mixedRows}
        getRowKey={(row) => row.id}
        onRowClick={onRowClick}
        isRowClickable={(row) => row.enabled}
      />,
    );

    const enabledRow = screen.getByRole("button", { name: /enabled row/i });
    const disabledRowText = screen.getByText("Disabled row");

    expect(enabledRow).toHaveAttribute("tabIndex", "0");
    expect(disabledRowText.closest("tr")).not.toHaveAttribute("role", "button");

    fireEvent.click(disabledRowText);
    expect(onRowClick).not.toHaveBeenCalled();

    fireEvent.click(enabledRow);
    expect(onRowClick).toHaveBeenCalledWith(mixedRows[0], enabledRow);
  });

  it("does not trigger a row action from nested interactive controls", () => {
    const onRowClick = vi.fn();
    const actionColumns = [
      ...columns,
      {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        render: () => <button type="button">Nested action</button>,
      },
    ];

    render(
      <DataTable
        columns={actionColumns}
        data={rows}
        getRowKey={(row) => row.id}
        onRowClick={onRowClick}
      />,
    );

    const nestedButton = screen.getByRole("button", { name: "Nested action" });
    fireEvent.click(nestedButton);
    fireEvent.keyDown(nestedButton, { key: "Enter" });
    fireEvent.keyDown(nestedButton, { key: " " });

    expect(onRowClick).not.toHaveBeenCalled();

    const row = screen.getByRole("button", { name: /persistent row/i });
    fireEvent.click(within(row).getByText("Persistent row"));
    expect(onRowClick).toHaveBeenCalledWith(rows[0], row);
  });

  it("keeps sort controls active for paginated tables with controlled sorting", () => {
    const onSortChange = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        sortState={null}
        onSortChange={onSortChange}
        pagination={{
          page: 0,
          pageSize: 25,
          total: 1,
          onPageChange: vi.fn(),
          onPageSizeChange: vi.fn(),
        }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /name/i }));

    expect(onSortChange).toHaveBeenCalledWith({ key: "name", dir: "asc" });
  });

  it("advances controlled sorting from ascending to descending", () => {
    const onSortChange = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        sortState={{ key: "name", dir: "asc" }}
        onSortChange={onSortChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /name/i }));

    expect(onSortChange).toHaveBeenCalledWith({ key: "name", dir: "desc" });
  });

  it("clears controlled sorting after descending order", () => {
    const onSortChange = vi.fn();

    render(
      <DataTable
        columns={columns}
        data={rows}
        getRowKey={(row) => row.id}
        sortState={{ key: "name", dir: "desc" }}
        onSortChange={onSortChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /name/i }));

    expect(onSortChange).toHaveBeenCalledWith(null);
  });

  it("opens faceted column filters and reports selected values", () => {
    const onColumnFilterChange = vi.fn();
    const facetedColumns = [
      {
        key: "name",
        label: "Name",
        filterOptions: [
          { value: "Persistent row", label: "Persistent row", count: 1 },
          { value: "Other row", label: "Other row", count: 2 },
        ],
      },
    ];

    render(
      <DataTable
        columns={facetedColumns}
        data={rows}
        getRowKey={(row) => row.id}
        columnFilters={{ name: [] }}
        onColumnFilterChange={onColumnFilterChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /filter.*name/i }));
    const dialog = screen.getByRole("dialog");
    const trigger = screen.getByRole("button", { name: /filter.*name/i });
    expect(trigger).toHaveAttribute("data-column-filter-selected-count", "0");
    expect(within(dialog).getByText("Persistent row")).toBeInTheDocument();

    const filterSearch = within(dialog).getByPlaceholderText(/search values/i);
    expect(filterSearch).toHaveAttribute("data-column-filter-search", "true");
    fireEvent.change(filterSearch, {
      target: { value: "other" },
    });
    expect(within(dialog).queryByText("Persistent row")).not.toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: /other row/i }));

    expect(onColumnFilterChange).toHaveBeenCalledWith("name", ["Other row"]);
  });

  it("marks selected filter options and exposes a clear action", () => {
    const onColumnFilterChange = vi.fn();

    render(
      <DataTable
        columns={[
          {
            key: "status",
            label: "Status",
            filterOptions: [{ value: "active", label: "Active", count: 1 }],
          },
        ]}
        data={rows}
        getRowKey={(row) => row.id}
        columnFilters={{ status: ["active"] }}
        onColumnFilterChange={onColumnFilterChange}
      />,
    );

    const trigger = screen.getByRole("button", { name: /filter.*status/i });
    expect(trigger).toHaveAttribute("data-column-filter-selected-count", "1");

    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog");
    const selectedOption = within(dialog).getByRole("button", { name: /active/i });
    expect(selectedOption).toHaveAttribute("data-column-filter-option-selected", "true");

    fireEvent.click(within(dialog).getByRole("button", { name: /clear.*status.*filter/i }));
    expect(onColumnFilterChange).toHaveBeenCalledWith("status", []);
  });

  const multiColumns = [
    { key: "name", label: "Name" },
    { key: "status", label: "Status" },
    { key: "owner", label: "Owner" },
  ];
  const multiRows = [{ id: "row-1", name: "Alpha", status: "Active", owner: "Sam" }];

  it("hides a column's header and cells when it is in the hidden set", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    expect(screen.queryByRole("columnheader", { name: /status/i })).not.toBeInTheDocument();
    expect(screen.queryByText("Active")).not.toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: /owner/i })).toBeInTheDocument();
    expect(screen.getByText("Sam")).toBeInTheDocument();
  });

  it("keeps non-hideable columns visible and listed as fixed in column settings", () => {
    columnVisibilityState.hiddenColumns = new Set(["name", "status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    expect(screen.getByRole("columnheader", { name: /name/i })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: /status/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");

    expect(within(dialog).getByText("Name")).toBeInTheDocument();
    expect(within(dialog).getByText("Fixed")).toBeInTheDocument();
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /name/i })).toBeDisabled();
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /status/i })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });

  it("does not expose a filter button for a hidden column", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={[
          { key: "name", label: "Name" },
          {
            key: "status",
            label: "Status",
            filterOptions: [{ value: "Active", label: "Active" }],
          },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
        columnFilters={{ status: [] }}
        onColumnFilterChange={vi.fn()}
      />,
    );

    expect(screen.queryByRole("button", { name: /filter.*status/i })).not.toBeInTheDocument();
  });

  it("keeps columns with active filters visible in column settings", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={[
          { key: "name", label: "Name" },
          {
            key: "status",
            label: "Status",
            filterOptions: [{ value: "Active", label: "Active" }],
          },
          { key: "owner", label: "Owner" },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
        columnFilters={{ status: ["Active"] }}
        onColumnFilterChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const statusToggle = within(dialog).getByRole("menuitemcheckbox", { name: /status/i });

    expect(screen.getByRole("columnheader", { name: /status/i })).toBeInTheDocument();
    expect(statusToggle).toBeDisabled();
    expect(statusToggle).toHaveAttribute("aria-checked", "true");
    expect(within(statusToggle).getByText("Filtered")).toBeInTheDocument();

    fireEvent.click(statusToggle);
    expect(columnVisibilityState.setHidden).not.toHaveBeenCalled();
  });

  it("keeps columns with active sorting visible in column settings", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
        sortState={{ key: "status", dir: "asc" }}
        onSortChange={vi.fn()}
      />,
    );

    expect(screen.getByRole("columnheader", { name: /status/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const statusToggle = within(dialog).getByRole("menuitemcheckbox", { name: /status/i });

    expect(statusToggle).toBeDisabled();
    expect(statusToggle).toHaveAttribute("aria-checked", "true");
    expect(within(statusToggle).getByText("Sorted")).toBeInTheDocument();

    fireEvent.click(statusToggle);
    expect(columnVisibilityState.setHidden).not.toHaveBeenCalled();
  });

  it("keeps active filter controls available while filter options are unresolved", () => {
    const onColumnFilterChange = vi.fn();

    render(
      <DataTable
        columns={[
          { key: "name", label: "Name" },
          {
            key: "status",
            label: "Status",
            filterOptions: [],
          },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        columnFilters={{ status: ["staged"] }}
        onColumnFilterChange={onColumnFilterChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /filter.*status/i }));
    const dialog = screen.getByRole("dialog");

    expect(within(dialog).getByText("staged")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: /clear.*status.*filter/i }));
    expect(onColumnFilterChange).toHaveBeenCalledWith("status", []);
  });

  it("lists hide-able columns in the column-settings popover and toggles them", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    // "name" is the first column and non-hideable by default; "status"/"owner" listed.
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /status/i })).toBeInTheDocument();
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /owner/i })).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("menuitemcheckbox", { name: /status/i }));

    expect(columnVisibilityState.setHidden).toHaveBeenCalledTimes(1);
    expect(columnVisibilityState.setHidden).toHaveBeenCalledWith(new Set(["status"]));
  });

  it("lists the non-hideable first column as a fixed picker entry", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /name/i })).toBeDisabled();
    expect(within(dialog).getByText("Fixed")).toBeInTheDocument();
  });

  it("keeps visible headers in parity with checked settings entries", () => {
    columnVisibilityState.hiddenColumns = new Set(["status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const visibleHeaderNames = screen
      .getAllByRole("columnheader")
      .map((header) => header.textContent?.trim())
      .filter((value): value is string => Boolean(value));
    const checkedEntryNames = within(dialog)
      .getAllByRole("menuitemcheckbox")
      .filter((entry) => entry.getAttribute("aria-checked") === "true")
      .map((entry) => entry.textContent?.replace("Fixed", "").trim())
      .filter((value): value is string => Boolean(value));

    expect(visibleHeaderNames.sort()).toEqual(checkedEntryNames.sort());
    expect(within(dialog).getByRole("menuitemcheckbox", { name: /status/i })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });

  it("ignores stale and non-hideable hidden keys when computing visible columns", () => {
    columnVisibilityState.hiddenColumns = new Set(["status", "ghost", "name"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    expect(screen.getByRole("columnheader", { name: /name/i })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: /status/i })).not.toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: /owner/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("2 shown")).toBeInTheDocument();
  });

  it("does not expose settings-excluded utility columns in the column picker", () => {
    render(
      <DataTable
        columns={[
          ...multiColumns,
          { key: "details", label: "", excludeFromSettings: true },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("menuitemcheckbox", { name: /details/i })).not.toBeInTheDocument();
    expect(within(dialog).getByText("3 shown")).toBeInTheDocument();
  });

  it("ignores hidden settings for settings-excluded utility columns", () => {
    columnVisibilityState.hiddenColumns = new Set(["details"]);

    render(
      <DataTable
        columns={[
          ...multiColumns,
          {
            key: "details",
            label: "",
            excludeFromSettings: true,
            render: () => <button type="button">Open details</button>,
          },
        ]}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    expect(screen.getByRole("button", { name: /open details/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("menuitemcheckbox", { name: /details/i })).not.toBeInTheDocument();
    expect(within(dialog).getByText("3 shown")).toBeInTheDocument();
  });

  it("throws when a settings-enabled table has an unlabeled column without an explicit opt-out", () => {
    expect(() =>
      render(
        <DataTable
          columns={[
            { key: "name", label: "Name" },
            { key: "details", label: "" },
          ]}
          data={rows}
          getRowKey={(row) => row.id}
          tableId="test-table"
        />,
      ),
    ).toThrow(/must either define a label or set excludeFromSettings: true/i);
  });

  it("throws when a table receives duplicate column keys", () => {
    expect(() =>
      render(
        <DataTable
          columns={[
            { key: "name", label: "Name" },
            { key: "name", label: "Display name" },
          ]}
          data={rows}
          getRowKey={(row) => row.id}
          tableId="test-table"
        />,
      ),
    ).toThrow(/duplicate column keys: name/i);
  });

  it("throws when a settings-enabled table has duplicate managed labels", () => {
    expect(() =>
      render(
        <DataTable
          columns={[
            { key: "address", label: "Name" },
            { key: "dns", label: "Name" },
          ]}
          data={rows}
          getRowKey={(row) => row.id}
          tableId="test-table"
        />,
      ),
    ).toThrow(/duplicate managed column labels/i);
  });

  it("throws when duplicate managed labels differ only by surrounding whitespace", () => {
    expect(() =>
      render(
        <DataTable
          columns={[
            { key: "address", label: "Name" },
            { key: "dns", label: " Name " },
          ]}
          data={rows}
          getRowKey={(row) => row.id}
          tableId="test-table"
        />,
      ),
    ).toThrow(/duplicate managed column labels/i);
  });

  it("disables hiding the last remaining visible column", () => {
    // With no non-hideable columns, hide every column except "owner" so it is
    // the sole visible column. Its checkbox must be disabled to keep >=1 visible.
    columnVisibilityState.hiddenColumns = new Set(["name", "status"]);

    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
        nonHideableKeys={[]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    const ownerToggle = within(dialog).getByRole("menuitemcheckbox", { name: /owner/i });
    expect(ownerToggle).toBeDisabled();

    fireEvent.click(ownerToggle);
    expect(columnVisibilityState.setHidden).not.toHaveBeenCalled();
  });

  it("does not render the column-settings button when tableId is absent", () => {
    render(
      <DataTable columns={multiColumns} data={multiRows} getRowKey={(row) => row.id} />,
    );

    expect(screen.queryByRole("button", { name: /columns/i })).not.toBeInTheDocument();
  });

  it("invokes reset from the column-settings popover", () => {
    render(
      <DataTable
        columns={multiColumns}
        data={multiRows}
        getRowKey={(row) => row.id}
        tableId="test-table"
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /columns/i }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /reset/i }));

    expect(columnVisibilityState.reset).toHaveBeenCalledTimes(1);
  });
});
