import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import WebhookConditionsEditor from "./WebhookConditionsEditor";
import type { ConditionLogic, ConditionRow } from "./webhookEvents";
import type * as ReactI18next from "react-i18next";
vi.mock("react-i18next", async original => ({ ...(await original<typeof ReactI18next>()), useTranslation: () => ({ t: (key: string) => key }) }));
function Editor() {
  const [value, setValue] = useState<{ logic: ConditionLogic; rows: ConditionRow[] }>({
    logic: "and", rows: [{ id: "initial", attr: "status", op: "eq", value: "active" }],
  });
  return <WebhookConditionsEditor {...value} onChange={setValue} />;
}
describe("WebhookConditionsEditor", () => {
  it("keeps invalid raw JSON editable and applies valid conditions to labeled fields", () => {
    render(<Editor />);
    expect(screen.getByRole("textbox", { name: "webhooks.conditions.attrLabel" })).toHaveValue("status");
    fireEvent.click(screen.getByRole("button", { name: "webhooks.conditions.editRaw" }));
    const raw = screen.getByRole("textbox", { name: "webhooks.conditions.rawLabel" });
    fireEvent.change(raw, { target: { value: "{" } });
    fireEvent.click(screen.getByRole("button", { name: "webhooks.conditions.applyRaw" }));
    expect(screen.getByRole("alert")).toHaveTextContent("webhooks.conditions.rawInvalid");
    expect(raw).toHaveValue("{");
    fireEvent.change(raw, { target: { value: JSON.stringify({ or: [{ attr: "name", op: "eq", value: "router" }] }) } });
    fireEvent.click(screen.getByRole("button", { name: "webhooks.conditions.applyRaw" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "webhooks.conditions.attrLabel" })).toHaveValue("name");
    expect(screen.getByRole("textbox", { name: "webhooks.conditions.valueLabel" })).toHaveValue("router");
    fireEvent.click(screen.getByRole("button", { name: "webhooks.conditions.removeRow" }));
    expect(screen.queryByRole("textbox", { name: "webhooks.conditions.attrLabel" })).not.toBeInTheDocument();
  });
});
