import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, validateEntityDraft } from "@/shared/forms/entityForm";
import type { InventoryEntityKey } from "@/shared/api/types";
import i18n from "@/shared/i18n";

function Form({ entity }: { entity: InventoryEntityKey }) {
  const [draft, setDraft] = useState(buildEntityDraft(entity));
  return <AllPermissions><EntityFormBody entity={entity} draft={draft}
    errors={validateEntityDraft(entity, draft, key => String(i18n.t(key)))}
    onField={(key, value) => setDraft(current => ({ ...current, [key]: value }))}
    loadOptions={() => []} onRequestCreate={vi.fn()} /></AllPermissions>;
}

describe("FHRP forms", () => {
  it("shows an optional name without a required marker or name error", () => {
    render(<Form entity="FHRP Groups" />);
    const name = screen.getByRole("textbox", { name: "Name" });
    expect(name).not.toBeRequired();
    expect(name.closest('[data-slot="form-field"]')).not.toHaveTextContent("Required");
    expect(screen.queryByText("Group name is required")).not.toBeInTheDocument();
  });
  it("masks authentication keys and accepts group number zero", () => {
    render(<Form entity="FHRP Groups" />);
    expect(screen.getByRole("combobox", { name: "Authentication Key" })).toHaveTextContent("Keep unchanged");
    expect(screen.queryByRole("textbox", { name: "Authentication Key" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Authentication Key" }), { key: "ArrowDown" });
    fireEvent.click(screen.getByRole("option", { name: "Set key" }));
    const key = screen.getByRole("textbox", { name: "Authentication Key" });
    fireEvent.change(key, { target: { value: "synthetic-key" } });
    expect(key).toHaveProperty("style.WebkitTextSecurity", "disc");
    const number = screen.getByRole("spinbutton", { name: "Group Number" });
    fireEvent.change(number, { target: { value: "0" } });
    expect(number).toHaveValue(0);
    expect(screen.queryByText("Group Number: Required")).not.toBeInTheDocument();
  });

  it("has no phantom assignment name and constrains priority to a byte", () => {
    render(<Form entity="FHRP Group Assignments" />);
    expect(screen.queryByRole("textbox", { name: "Name" })).not.toBeInTheDocument();
    const priority = screen.getByRole("spinbutton", { name: "Priority" });
    expect(priority).toHaveAttribute("min", "0");
    expect(priority).toHaveAttribute("max", "255");
    fireEvent.change(priority, { target: { value: "256" } });
    expect(priority).toBeInvalid();
    fireEvent.change(priority, { target: { value: "255" } });
    expect(priority).toBeValid();
  });

  it("clears both halves of an optional IP assignment", () => {
    const onField = vi.fn();
    render(<AllPermissions><EntityFormBody entity="IP Addresses"
      draft={{ prefix: "192.0.2.1/24", assignedObject: "group", assignedObject__type: "ipam.fhrpgroup" }}
      onField={onField} loadOptions={() => [{ id: "group", name: "Gateway" }]} onRequestCreate={vi.fn()} /></AllPermissions>);
    fireEvent.click(screen.getByRole("button", { name: "Clear Assigned Object" }));
    expect(onField.mock.calls).toEqual([["assignedObject__type", ""], ["assignedObject", ""]]);
  });
});
