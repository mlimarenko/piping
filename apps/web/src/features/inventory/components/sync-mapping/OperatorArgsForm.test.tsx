import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { changeAppLanguage } from "@/shared/i18n";
import en from "@/shared/i18n/locales/en";
import OperatorArgsForm from "./OperatorArgsForm";

beforeEach(async () => { await changeAppLanguage("en"); });

describe("mapping argument fields", () => {
  it.each(["trim", "lower", "upper", "mac_normalize", "ipv4_prefix_canonical"] as const)(
    "%s keeps the associated field label, argument updates and readonly state", operator => {
      const onChange = vi.fn();
      const { rerender } = render(<OperatorArgsForm operator={operator} args={{ field: "name", retained: true }} onChange={onChange} />);
      const input = screen.getByRole("textbox", { name: new RegExp(en.sync.mapping.args.fieldPath) });
      expect(input).toHaveValue("name");
      fireEvent.change(input, { target: { value: "device.name" } });
      expect(onChange).toHaveBeenCalledWith({ field: "device.name", retained: true });
      rerender(<OperatorArgsForm operator={operator} args={{ field: "device.name" }} onChange={onChange} disabled />);
      expect(input).toBeDisabled();
    },
  );

  it("keeps split-index fields labelled and updates only the selected argument", () => {
    const onChange = vi.fn();
    const args = { field: "name", separator: "-", index: 0 };
    render(<OperatorArgsForm operator="split_index" args={args} onChange={onChange} />);
    fireEvent.change(screen.getByRole("textbox", { name: new RegExp(en.sync.mapping.args.separator) }), { target: { value: "/" } });
    expect(onChange).toHaveBeenLastCalledWith({ ...args, separator: "/" });
    fireEvent.change(screen.getByRole("spinbutton", { name: en.sync.mapping.args.index }), { target: { value: "2" } });
    expect(onChange).toHaveBeenLastCalledWith({ ...args, index: 2 });
  });
});
