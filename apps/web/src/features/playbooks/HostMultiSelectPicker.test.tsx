import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import "@/shared/i18n";
import type { ConnectionTarget, Paginated } from "@/shared/api/types";
import type { Uuid } from "@/shared/lib/ids";
import HostMultiSelectPicker from "./HostMultiSelectPicker";
import type { SelectedHost } from "./hostSelection";

const api = vi.hoisted(() => ({ listConnectionTargetPage: vi.fn() }));

vi.mock("@/features/connections/api", () => ({
  listConnectionTargetPage: api.listConnectionTargetPage,
}));

type TargetOverrides = Omit<Partial<ConnectionTarget>, "id" | "name"> & {
  id: string;
  name: string;
};

function target(over: TargetOverrides): ConnectionTarget {
  return {
    targetKind: "device",
    isCctv: false,
    addressBound: true,
    primaryIp: "10.0.0.1",
    ...over,
    id: over.id as Uuid,
  };
}

function page(items: ConnectionTarget[], total: number, offset = 0, limit = 50): Paginated<ConnectionTarget> {
  return { items, total, limit, offset };
}

function selectedHost(over: Partial<SelectedHost> & { id: string; name: string }): SelectedHost {
  return { subtitle: "", sourceKind: "device", sourceId: null, ...over };
}

function renderPicker(over: Partial<ComponentProps<typeof HostMultiSelectPicker>> = {}) {
  const onChange = vi.fn<(next: SelectedHost[]) => void>();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={queryClient}>
      <HostMultiSelectPicker selected={over.selected ?? []} onChange={over.onChange ?? onChange} />
    </QueryClientProvider>,
  );
  return { ...view, onChange };
}

beforeEach(() => {
  api.listConnectionTargetPage.mockReset();
  api.listConnectionTargetPage.mockResolvedValue(page([], 0));
});

describe("host picker server search", () => {
  it("searches on the server and reaches a host beyond the first page", async () => {
    api.listConnectionTargetPage.mockImplementation(({ offset }: { offset: number }) =>
      Promise.resolve(
        offset === 0
          ? page([target({ id: "00000000-0000-4000-8000-000000000001", name: "edge-1" })], 120)
          : page(
              [target({ id: "00000000-0000-4000-8000-000000000099", name: "edge-250" })],
              120,
              offset,
            ),
      ),
    );
    renderPicker();

    expect(await screen.findByText("edge-1")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Name or address"), { target: { value: "edge-250" } });

    await waitFor(() =>
      expect(api.listConnectionTargetPage).toHaveBeenCalledWith(
        expect.objectContaining({ search: "edge-250", offset: 0 }),
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: "Next page" }));

    expect(await screen.findByText("edge-250")).toBeInTheDocument();
    expect(api.listConnectionTargetPage).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 50, limit: 50 }),
    );
  });

  it("keeps a blocked host visible but not selectable", async () => {
    api.listConnectionTargetPage.mockResolvedValue(
      page(
        [
          target({ id: "00000000-0000-4000-8000-000000000001", name: "web-1" }),
          target({
            id: "00000000-0000-4000-8000-000000000002",
            name: "db-1",
            addressBound: false,
          }),
        ],
        2,
      ),
    );
    const { onChange } = renderPicker();

    const blocked = await screen.findByRole("button", { name: /db-1/ });
    expect(blocked).toBeDisabled();
    fireEvent.click(blocked);
    expect(onChange).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /web-1/ }));
    expect(onChange).toHaveBeenCalledWith([
      expect.objectContaining({ id: "00000000-0000-4000-8000-000000000001", name: "web-1" }),
    ]);
  });

  it("shows the selection as its own list independent of the current page", async () => {
    renderPicker({
      selected: [selectedHost({ id: "00000000-0000-4000-8000-000000000007", name: "kept-host" })],
    });

    expect(await screen.findByText("kept-host")).toBeInTheDocument();
    expect(await screen.findByText("No matching hosts.")).toBeInTheDocument();
  });
});

describe("host picker select all", () => {
  it("walks every page and adds only bound, deduplicated hosts", async () => {
    const bound = Array.from({ length: 248 }, (_, index) =>
      target({
        id: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
        name: `host-${index}`,
      }),
    );
    const firstPage = [
      ...bound,
      target({ id: "00000000-0000-4000-8000-000000000901", name: "no-ip-1", addressBound: false }),
      target({ id: "00000000-0000-4000-8000-000000000902", name: "no-ip-2", addressBound: false }),
    ];
    const secondPage = [
      bound[0]!,
      target({ id: "00000000-0000-4000-8000-000000000903", name: "no-ip-3", addressBound: false }),
      ...Array.from({ length: 48 }, (_, index) =>
        target({
          id: `00000000-0000-4000-8000-${String(1000 + index).padStart(12, "0")}`,
          name: `host-${250 + index}`,
        }),
      ),
    ];

    api.listConnectionTargetPage.mockImplementation(({ limit, offset }: { limit: number; offset: number }) => {
      if (limit === 250) {
        return Promise.resolve(offset === 0 ? page(firstPage, 300, 0, 250) : page(secondPage, 300, 250, 250));
      }
      return Promise.resolve(page([bound[0]!], 300));
    });

    const { onChange } = renderPicker({
      selected: [selectedHost({ id: bound[0]!.id, name: bound[0]!.name })],
    });

    fireEvent.click(await screen.findByRole("button", { name: /Select all found \(300\)/ }));

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const next = onChange.mock.calls.at(-1)?.[0] as SelectedHost[];
    expect(next).toHaveLength(296);
    expect(new Set(next.map((host) => host.id)).size).toBe(296);
    expect(next.map((host) => host.name)).not.toContain("no-ip-1");
    expect(next.map((host) => host.name)).toContain("host-250");
  });

  it("does not apply a partial sweep when a page fails", async () => {
    api.listConnectionTargetPage.mockImplementation(({ limit, offset }: { limit: number; offset: number }) => {
      if (limit !== 250) return Promise.resolve(page([], 300));
      if (offset === 0) {
        return Promise.resolve(
          page([target({ id: "00000000-0000-4000-8000-000000000001", name: "host-0" })], 300, 0, 250),
        );
      }
      return Promise.reject(new Error("page failed"));
    });

    const { onChange } = renderPicker();
    fireEvent.click(await screen.findByRole("button", { name: /Select all found \(300\)/ }));

    expect(await screen.findByText("Could not finish; the selection was not changed.")).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /^retry$/i })).toBeInTheDocument();
  });

  it("applies nothing when the sweep is cancelled", async () => {
    const deferred: { resolve?: (value: Paginated<ConnectionTarget>) => void } = {};
    api.listConnectionTargetPage.mockImplementation(({ limit }: { limit: number }) => {
      if (limit !== 250) return Promise.resolve(page([], 10));
      return new Promise<Paginated<ConnectionTarget>>((resolve) => {
        deferred.resolve = resolve;
      });
    });

    const { onChange } = renderPicker();
    fireEvent.click(await screen.findByRole("button", { name: /Select all found \(10\)/ }));
    fireEvent.click(await screen.findByRole("button", { name: /^cancel$/i }));
    deferred.resolve?.(page([target({ id: "00000000-0000-4000-8000-000000000001", name: "late" })], 10));

    expect(await screen.findByText("Cancelled; the selection was not changed.")).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("host picker pasted list", () => {
  it("resolves names and addresses, and reports ambiguous, missing and unaddressed entries", async () => {
    const byTerm: Record<string, ConnectionTarget[]> = {
      "web-1": [target({ id: "00000000-0000-4000-8000-000000000001", name: "web-1" })],
      "10.0.0.10": [target({ id: "00000000-0000-4000-8000-000000000002", name: "ip-host", primaryIp: "10.0.0.10" })],
      duplicate: [
        target({ id: "00000000-0000-4000-8000-000000000003", name: "duplicate" }),
        target({ id: "00000000-0000-4000-8000-000000000004", name: "duplicate" }),
      ],
      "no-ip": [
        target({
          id: "00000000-0000-4000-8000-000000000005",
          name: "no-ip",
          addressBound: false,
        }),
      ],
    };

    api.listConnectionTargetPage.mockImplementation(({ limit, search }: { limit: number; search?: string }) => {
      if (limit !== 20) return Promise.resolve(page([], 0));
      const items = byTerm[search ?? ""] ?? [];
      return Promise.resolve(page(items, items.length));
    });

    const { onChange } = renderPicker();
    fireEvent.click(await screen.findByRole("button", { name: "Paste a list" }));
    fireEvent.change(screen.getByLabelText("Names or addresses, one per line"), {
      target: { value: "web-1\n10.0.0.10\nduplicate\nmissing\nno-ip" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add from list" }));

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const next = onChange.mock.calls.at(-1)?.[0] as SelectedHost[];
    expect(next.map((host) => host.name)).toEqual(["web-1", "ip-host"]);
    expect(await screen.findByText(/Not found \(1\):/)).toBeInTheDocument();
    expect(screen.getByText(/Ambiguous \(1\):/)).toBeInTheDocument();
    expect(screen.getByText(/Without a bound address \(1\):/)).toBeInTheDocument();
  });

  it("finds an exact host on a later server page instead of reporting it missing", async () => {
    const filler = Array.from({ length: 20 }, (_, index) =>
      target({
        id: `00000000-0000-4000-8000-${String(700 + index).padStart(12, "0")}`,
        name: `filler-${index}`,
      }),
    );
    const far = target({ id: "00000000-0000-4000-8000-000000000777", name: "far-host" });
    api.listConnectionTargetPage.mockImplementation(
      ({ limit, offset, search }: { limit: number; offset: number; search?: string }) => {
        if (limit !== 20 || search !== "far-host") return Promise.resolve(page([], 0));
        return Promise.resolve(offset === 0 ? page(filler, 25, 0, 20) : page([far], 25, 20, 20));
      },
    );

    const { onChange } = renderPicker();
    fireEvent.click(await screen.findByRole("button", { name: "Paste a list" }));
    fireEvent.change(screen.getByLabelText("Names or addresses, one per line"), {
      target: { value: "far-host" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add from list" }));

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const next = onChange.mock.calls.at(-1)?.[0] as SelectedHost[];
    expect(next.map((host) => host.name)).toEqual(["far-host"]);
    expect(api.listConnectionTargetPage).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 20, limit: 20, search: "far-host" }),
    );
    expect(screen.queryByText(/Not found \(1\):/)).not.toBeInTheDocument();
  });

  it("treats an exact duplicate on a later server page as ambiguous and selects neither", async () => {
    const first = target({ id: "00000000-0000-4000-8000-000000000801", name: "twin" });
    const second = target({ id: "00000000-0000-4000-8000-000000000802", name: "twin" });
    const filler = Array.from({ length: 19 }, (_, index) =>
      target({
        id: `00000000-0000-4000-8000-${String(900 + index).padStart(12, "0")}`,
        name: `other-${index}`,
      }),
    );
    api.listConnectionTargetPage.mockImplementation(
      ({ limit, offset, search }: { limit: number; offset: number; search?: string }) => {
        if (limit !== 20 || search !== "twin") return Promise.resolve(page([], 0));
        return Promise.resolve(
          offset === 0 ? page([first, ...filler], 25, 0, 20) : page([second], 25, 20, 20),
        );
      },
    );

    const { onChange } = renderPicker();
    fireEvent.click(await screen.findByRole("button", { name: "Paste a list" }));
    fireEvent.change(screen.getByLabelText("Names or addresses, one per line"), {
      target: { value: "twin" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add from list" }));

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    const next = onChange.mock.calls.at(-1)?.[0] as SelectedHost[];
    expect(next).toEqual([]);
    expect(await screen.findByText(/Ambiguous \(1\):/)).toBeInTheDocument();
    expect(api.listConnectionTargetPage).toHaveBeenCalledWith(
      expect.objectContaining({ offset: 20, search: "twin" }),
    );
  });
});
