import { useState } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { AuditObjectMultiPicker } from "./AuditObjectMultiPicker";
import type * as AuditApi from "@/features/audit/api";
import type { AuditLookupOption } from "@/shared/api/types";

const apiMocks = vi.hoisted(() => ({
  searchAuditTargets: vi.fn(),
}));

vi.mock("@/features/audit/api", async () => {
  const actual = await vi.importActual<typeof AuditApi>("@/features/audit/api");
  return { ...actual, searchAuditTargets: apiMocks.searchAuditTargets };
});

function makeOption(over: Partial<AuditLookupOption> = {}): AuditLookupOption {
  return {
    id: "a1",
    kind: "user",
    label: "Alice Admin",
    secondary: "alice@corp",
    target_type: "user",
    ...over,
  };
}

const alice = makeOption();
const bob = makeOption({ id: "b2", label: "Bob Builder", secondary: "bob@corp" });
const carol = makeOption({
  id: "c3",
  kind: "device",
  label: "Carol Auditor",
  secondary: "10.0.0.9",
  target_type: "device",
});

const kindLabel = (kind?: string): string =>
  kind === "user" ? "User" : kind === "device" ? "Device" : "";

const labelProps = {
  placeholder: "Any target",
  searchPlaceholder: "Search targets",
  emptyText: "No targets found",
  loadingText: "Loading targets",
  clearText: "Clear selection",
  groupLabel: "Targets",
  kindLabel,
};

/**
 * Stateful host so `onChange` genuinely feeds back into `value` — this lets us
 * assert on the resulting rendered selection (chips, trigger count, list check
 * state) instead of only on the callback shape.
 */
function Harness({
  initial = [],
  onChange,
}: {
  initial?: AuditLookupOption[];
  onChange?: (next: AuditLookupOption[]) => void;
}) {
  const [value, setValue] = useState<AuditLookupOption[]>(initial);
  return (
    <AuditObjectMultiPicker
      {...labelProps}
      value={value}
      onChange={(next) => {
        onChange?.(next);
        setValue(next);
      }}
    />
  );
}

function renderPicker(
  opts: { initial?: AuditLookupOption[]; onChange?: (next: AuditLookupOption[]) => void } = {},
): ReturnType<typeof render> {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <Harness initial={opts.initial} onChange={opts.onChange} />
    </QueryClientProvider>,
  );
}

function panel(): HTMLElement {
  const node = document.querySelector("[data-picker-panel]");
  if (!node) throw new Error("picker panel is not open");
  return node as HTMLElement;
}

describe("AuditObjectMultiPicker", () => {
  beforeEach(() => {
    apiMocks.searchAuditTargets.mockReset();
    apiMocks.searchAuditTargets.mockImplementation((q?: AuditApi.AuditLookupQuery) =>
      Promise.resolve(q?.search === "carol" ? [carol] : [alice, bob]),
    );
  });

  it("opens on trigger click and lists server-hydrated options with their kind/secondary line", async () => {
    renderPicker();

    // Closed to start: no panel, trigger shows the placeholder.
    expect(document.querySelector("[data-picker-panel]")).toBeNull();
    expect(screen.getByText("Any target")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("combobox"));

    expect(document.querySelector("[data-picker-panel]")).toBeInTheDocument();
    expect(await within(panel()).findByRole("option", { name: /Alice Admin/ })).toBeInTheDocument();
    expect(within(panel()).getByRole("option", { name: /Bob Builder/ })).toBeInTheDocument();
    // Secondary line composes `kindLabel(kind)` with the option's `secondary`.
    expect(within(panel()).getByText("User · alice@corp")).toBeInTheDocument();

    expect(apiMocks.searchAuditTargets).toHaveBeenCalledWith(
      expect.objectContaining({ targetType: undefined, search: undefined, limit: 30 }),
    );
  });

  it("selects an option, firing onChange with the value and rendering a removable chip", async () => {
    const onChange = vi.fn();
    renderPicker({ onChange });

    fireEvent.click(screen.getByRole("combobox"));
    fireEvent.click(await within(panel()).findByRole("option", { name: /Alice Admin/ }));

    expect(onChange).toHaveBeenCalledWith([alice]);
    // Trigger summarizes the count and a removable chip is now present.
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
  });

  it("deselects an already-selected option when re-clicked in the list", async () => {
    const onChange = vi.fn();
    renderPicker({ initial: [alice], onChange });

    expect(screen.getByText("1 selected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("combobox"));
    fireEvent.click(await within(panel()).findByRole("option", { name: /Alice Admin/ }));

    expect(onChange).toHaveBeenCalledWith([]);
    // Back to the empty state: placeholder trigger, no chips.
    expect(screen.getByText("Any target")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });

  it("renders one chip per selection and removes only the clicked chip's option", () => {
    const onChange = vi.fn();
    renderPicker({ initial: [alice, bob], onChange });

    // Chips render from `value` without opening the panel.
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    const removeButtons = screen.getAllByRole("button", { name: "Remove" });
    expect(removeButtons).toHaveLength(2);

    fireEvent.click(removeButtons[0]!); // Alice's chip (chips follow value order).

    expect(onChange).toHaveBeenCalledWith([bob]);
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
  });

  it("refetches with the debounced search term and swaps the option list", async () => {
    renderPicker();

    fireEvent.click(screen.getByRole("combobox"));
    await within(panel()).findByRole("option", { name: /Alice Admin/ });

    fireEvent.change(screen.getByPlaceholderText("Search targets"), {
      target: { value: "carol" },
    });

    await waitFor(() =>
      expect(apiMocks.searchAuditTargets).toHaveBeenCalledWith(
        expect.objectContaining({ search: "carol", limit: 30 }),
      ),
    );
    expect(await within(panel()).findByRole("option", { name: /Carol Auditor/ })).toBeInTheDocument();
    await waitFor(() =>
      expect(within(panel()).queryByRole("option", { name: /Alice Admin/ })).not.toBeInTheDocument(),
    );
  });

  it("clears the entire selection via the clear item", async () => {
    const onChange = vi.fn();
    renderPicker({ initial: [alice, bob], onChange });

    fireEvent.click(screen.getByRole("combobox"));
    fireEvent.click(await within(panel()).findByRole("option", { name: /Clear selection/ }));

    expect(onChange).toHaveBeenCalledWith([]);
    expect(screen.getByText("Any target")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });
});
