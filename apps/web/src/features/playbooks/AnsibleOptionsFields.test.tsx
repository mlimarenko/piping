import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AnsibleOptionsFields } from "./AnsibleOptionsFields";

function Editor() {
  const [value, setValue] = useState("[]");
  const [limit, setLimit] = useState("");
  return <>
    <AnsibleOptionsFields value={value} onChange={setValue} limit={limit} onLimitChange={setLimit} />
    <output aria-label="Saved arguments">{value}</output>
    <output aria-label="Saved limit">{limit}</output>
  </>;
}

describe("Ansible option fields", () => {
  it("edits options and clears become details when privilege escalation is disabled", () => {
    render(<Editor />);
    fireEvent.change(screen.getByRole("textbox", { name: /^tags/i }), { target: { value: "deploy,verify" } });
    fireEvent.change(screen.getByRole("textbox", { name: /^limit$/i }), { target: { value: "host*" } });
    fireEvent.click(screen.getByRole("switch", { name: /check mode/i }));
    fireEvent.click(screen.getByRole("switch", { name: /show differences/i }));
    expect(screen.getByText(/file contents/i)).toBeVisible();
    const become = screen.getByRole("switch", { name: /privilege escalation/i });
    fireEvent.click(become);
    fireEvent.change(screen.getByRole("textbox", { name: /become user/i }), { target: { value: "service" } });
    fireEvent.change(screen.getByRole("textbox", { name: /become method/i }), { target: { value: "sudo" } });
    expect(JSON.parse(screen.getByLabelText("Saved arguments").textContent)).toEqual([
      "--tags=deploy,verify", "--become-user=service", "--become-method=sudo", "--check", "--diff", "--become",
    ]);
    expect(screen.getByLabelText("Saved limit")).toHaveTextContent("host*");
    fireEvent.click(become);
    expect(screen.queryByRole("textbox", { name: /become user/i })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Saved arguments").textContent).not.toContain("become");
  });

  it("does not silently drop unsupported saved options", () => {
    const change = vi.fn();
    render(<AnsibleOptionsFields value='["--vault-id=prod@prompt"]' onChange={change} limit="" onLimitChange={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent(/unsupported or conflicting/i);
    expect(change).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /clear unsupported/i }));
    expect(change).toHaveBeenCalledWith("[]");
  });

  it("locks option and limit controls during submission", () => {
    render(<AnsibleOptionsFields value="[]" onChange={vi.fn()} limit="" onLimitChange={vi.fn()} disabled />);
    for (const field of [...screen.getAllByRole("textbox"), ...screen.getAllByRole("switch"), ...screen.getAllByRole("combobox")]) {
      expect(field).toBeDisabled();
    }
  });
});
