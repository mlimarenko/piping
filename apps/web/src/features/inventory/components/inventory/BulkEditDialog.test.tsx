import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import BulkEditDialog from "./BulkEditDialog";

describe("inventory bulk fields", () => {
  it.each(["Wireless LANs", "Wireless Links", "Interfaces"] as const)("does not expose credentials or endpoint-owned associations in %s bulk edits", (entity) => {
    render(<BulkEditDialog open onOpenChange={vi.fn()} entity={entity}
      selectedRows={[{ id: "wireless", name: "Wireless", type: "interface" }]}
      loadOptions={() => []} onApply={vi.fn()} onCreateRelated={vi.fn()} />);
    expect(screen.queryByRole("radiogroup", { name: "Pre-shared key" })).not.toBeInTheDocument();
    expect(screen.queryByRole("radiogroup", { name: "Wireless link" })).not.toBeInTheDocument();
    if (entity !== "Interfaces") expect(screen.getByRole("radiogroup", { name: "Status" })).toBeInTheDocument();
  });
  it.each([
    { entity: "Virtual Machines", labels: ["vCPUs", "Memory (MB)"], keys: ["vcpus", "memory"] },
    { entity: "Virtual Machine Types", labels: ["Default vCPUs", "Default memory (MB)"], keys: ["defaultVcpus", "defaultMemory"] },
  ] as const)("offers explicit clears only for nullable $entity numbers", ({ entity, labels, keys }) => {
    const apply = vi.fn();
    render(<BulkEditDialog open onOpenChange={vi.fn()} entity={entity}
      selectedRows={[{ id: "vm", name: "VM", type: "vm", vcpus: "1.5", memory: "2048" }]}
      loadOptions={() => []} onApply={apply} onCreateRelated={vi.fn()} />);
    for (const label of labels) {
      const group = screen.getByRole("radiogroup", { name: label });
      fireEvent.click(within(group).getByRole("radio", { name: "Clear" }));
    }
    if (entity === "Virtual Machines") {
      const disk = screen.getByRole("radiogroup", { name: "Disk (GB)" });
      expect(within(disk).queryByRole("radio", { name: "Clear" })).toBeNull();
    }
    fireEvent.click(screen.getByRole("button", { name: "Apply to 1" }));
    expect(apply).toHaveBeenCalledWith(Object.fromEntries(keys.map((key) => [key, null])));
  });
});
