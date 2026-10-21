import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import PermissionPicker, { type PermissionBundle } from "@/shared/ui-shared/PermissionPicker";

const GROUPS = [
  {
    key: "inventory",
    title: "inventory",
    items: [
      { value: "read:inventory", label: "read" },
      { value: "create:inventory", label: "create" },
    ],
  },
];

describe("PermissionPicker", () => {
  it("toggles a grid value", () => {
    const onToggleValue = vi.fn();
    render(
      <PermissionPicker
        groups={GROUPS}
        selectedValues={new Set()}
        onToggleValue={onToggleValue}
        countLabel="0 selected"
        hasSelection={false}
        emptyLabel="none"
      />,
    );
    fireEvent.click(screen.getByText("read"));
    expect(onToggleValue).toHaveBeenCalledWith("read:inventory");
  });

  it("renders disabled values as visible non-clickable checkboxes", () => {
    const onToggleValue = vi.fn();
    render(
      <PermissionPicker
        groups={[
          {
            key: "inventory",
            title: "inventory",
            items: [{ value: "write:inventory", label: "write", disabled: true }],
          },
        ]}
        selectedValues={new Set()}
        onToggleValue={onToggleValue}
        countLabel="0 selected"
        hasSelection={false}
        emptyLabel="none"
      />,
    );

    const checkbox = screen.getByRole("checkbox", { name: "write" });
    expect(checkbox).toBeDisabled();

    fireEvent.click(screen.getByText("write"));
    expect(onToggleValue).not.toHaveBeenCalled();
  });

  it("shows clear-all only with a selection and fires it", () => {
    const onClearAll = vi.fn();
    const { rerender } = render(
      <PermissionPicker
        groups={GROUPS}
        selectedValues={new Set()}
        onToggleValue={() => {}}
        countLabel="0 selected"
        hasSelection={false}
        onClearAll={onClearAll}
        clearAllLabel="Clear all"
        emptyLabel="none"
      />,
    );
    expect(screen.queryByRole("button", { name: "Clear all" })).toBeNull();

    rerender(
      <PermissionPicker
        groups={GROUPS}
        selectedValues={new Set(["read:inventory"])}
        onToggleValue={() => {}}
        countLabel="1 selected"
        hasSelection
        onClearAll={onClearAll}
        clearAllLabel="Clear all"
        emptyLabel="none"
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
    expect(onClearAll).toHaveBeenCalledTimes(1);
  });

  it("renders bundle pills and reports the toggled bundle", () => {
    const onToggleBundle = vi.fn();
    const bundles: PermissionBundle[] = [
      { id: "readOnly", label: "Read only", values: ["read:inventory"] },
    ];
    render(
      <PermissionPicker
        groups={GROUPS}
        selectedValues={new Set()}
        onToggleValue={() => {}}
        countLabel="0 selected"
        hasSelection={false}
        bundles={bundles}
        isBundleActive={() => false}
        onToggleBundle={onToggleBundle}
        emptyLabel="none"
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Read only/ }));
    expect(onToggleBundle).toHaveBeenCalledWith(bundles[0]);
  });
});
