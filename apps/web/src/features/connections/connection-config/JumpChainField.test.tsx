import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";
import "@/shared/i18n";
import JumpChainField from "./JumpChainField";
import type { ConnConfJumpHop, ConnConfRecord } from "./api";

const record = (id: string, name: string, protocol = "ssh", enabled = true) =>
  ({ id, name, protocol, enabled }) as ConnConfRecord;
const records = [record("self", "Self"), record("a", "Alpha"), record("b", "Beta"), record("off", "Disabled", "ssh", false), record("rdp", "RDP", "rdp")];

function Fixture({ initial = [], choices = records }: { initial?: ConnConfJumpHop[]; choices?: ConnConfRecord[] }) {
  const [value, setValue] = useState(initial);
  return <JumpChainField value={value} records={choices} recordId="self" onChange={setValue} />;
}

it("adds only eligible records, reorders and removes hops", () => {
  render(<Fixture />);
  const picker = screen.getByRole("combobox", { name: "Add SSH jump hop" });
  fireEvent.click(picker);
  expect(screen.queryByRole("option", { name: "Self" })).not.toBeInTheDocument();
  expect(screen.queryByRole("option", { name: "Disabled" })).not.toBeInTheDocument();
  expect(screen.queryByRole("option", { name: "RDP" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("option", { name: "Alpha" }));
  fireEvent.click(picker);
  expect(screen.queryByRole("option", { name: "Alpha" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("option", { name: "Beta" }));
  fireEvent.click(screen.getByRole("button", { name: "Move Beta up" }));
  const list = screen.getByRole("list");
  expect(within(list).getAllByRole("listitem").map(item => item.textContent)).toEqual([expect.stringContaining("Beta"), expect.stringContaining("Alpha")]);
  fireEvent.click(screen.getByRole("button", { name: "Remove Beta" }));
  expect(within(list).getAllByRole("listitem")).toHaveLength(1);
});

it("disables known nested routes while permitting inherited direct and empty overrides", () => {
  const choices = [
    { ...record("nested", "Nested"), params: { jump: [{ connconf_record_id: "a" }] } },
    { ...record("inherited", "Inherited nested"), params: {}, base_record_id: "nested" },
    { ...record("empty", "Empty override"), params: { jump: [] }, base_record_id: "nested" },
    { ...record("base", "Direct base"), params: {} },
    { ...record("direct", "Inherited direct"), params: {}, base_record_id: "base" },
  ];
  render(<Fixture choices={choices} />);
  const picker = screen.getByRole("combobox", { name: "Add SSH jump hop" });
  fireEvent.click(picker);
  const nested = screen.getByRole("option", { name: "Nested: nested jump chains are unsupported" });
  expect(nested).toHaveAttribute("aria-disabled", "true");
  expect(screen.getByRole("option", { name: "Inherited nested: nested jump chains are unsupported" })).toHaveAttribute("aria-disabled", "true");
  fireEvent.click(nested);
  expect(screen.queryByRole("button", { name: /Remove Nested/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("option", { name: "Inherited direct" }));
  expect(screen.getByRole("button", { name: "Remove Inherited direct" })).toBeInTheDocument();
  fireEvent.click(picker);
  fireEvent.click(screen.getByRole("option", { name: "Empty override" }));
  expect(screen.getByRole("button", { name: "Remove Empty override" })).toBeInTheDocument();
});

it("retains and removes an existing unsupported nested hop", () => {
  const choices = [{ ...record("nested", "Nested"), params: { jump: [{ connconf_record_id: "a" }] } }];
  render(<Fixture choices={choices} initial={[{ connconf_record_id: "nested" }]} />);
  expect(screen.getByText(/Nested: nested jump chains are unsupported/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Remove Nested: nested jump chains are unsupported" }));
  expect(screen.queryByRole("button", { name: /Remove Nested/ })).not.toBeInTheDocument();
});

it("keeps unavailable hops visible and removable", () => {
  render(<Fixture initial={[{ connconf_record_id: "gone" }, { connconf_record_id: "off" }]} />);
  expect(screen.getByText(/Unavailable: gone/)).toBeInTheDocument();
  expect(screen.getByText(/Unavailable: off/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Remove Unavailable: gone" }));
  expect(screen.queryByText(/Unavailable: gone/)).not.toBeInTheDocument();
  expect(screen.getByText(/Unavailable: off/)).toBeInTheDocument();
});
