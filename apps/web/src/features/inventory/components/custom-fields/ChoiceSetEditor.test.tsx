import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import "@/shared/i18n";
import { describe, expect, it, vi } from "vitest";

import ChoiceSetEditor from "./ChoiceSetEditor";

function renderEditor(onCreate = vi.fn(), onOpenChange = vi.fn()) {
  render(
    <ChoiceSetEditor
      open
      onOpenChange={onOpenChange}
      saving={false}
      onCreate={onCreate}
      onUpdate={vi.fn()}
    />,
  );
  return { onCreate, onOpenChange };
}

describe("ChoiceSetEditor", () => {
  it("hosts the persisted form as an editor page rather than a modal dialog", () => {
    renderEditor();

    expect(screen.getByTestId("entity-editor-page")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("persists the trimmed payload and closes through the host", () => {
    const { onCreate, onOpenChange } = renderEditor();
    const boxes = screen.getAllByRole("textbox");

    fireEvent.change(boxes[0]!, { target: { value: "  Environments  " } });
    fireEvent.change(boxes[2]!, { target: { value: "prod" } });
    fireEvent.click(screen.getByRole("button", { name: /create/i }));

    expect(onCreate).toHaveBeenCalledWith({
      name: "Environments",
      description: "",
      order_alphabetically: false,
      extra_choices: [{ value: "prod", label: "prod" }],
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("announces name and choices errors and marks the name input invalid", async () => {
    const { onCreate } = renderEditor();

    fireEvent.click(screen.getByRole("button", { name: /create/i }));

    const alerts = await screen.findAllByRole("alert");
    expect(alerts.map((a) => a.textContent)).toEqual(
      expect.arrayContaining([
        "Name is required",
        "At least one choice with a value is required",
      ]),
    );
    expect(onCreate).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: /Name/ })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("textbox", { name: /Name/ })).toHaveAccessibleDescription("Name is required");
    expect(screen.getByRole("group", { name: "Choices" })).toHaveAccessibleDescription("At least one choice with a value is required");
    fireEvent.change(screen.getByRole("textbox", { name: /Name/ }), { target: { value: "Environments" } });
    expect(screen.getByRole("textbox", { name: /Name/ })).not.toHaveAttribute("aria-describedby");
  });

  it("routes the cancel affordance through the editor cancel contract", () => {
    const { onOpenChange } = renderEditor();
    const cancel = document.querySelector<HTMLButtonElement>("[data-editor-cancel]");
    if (!cancel) throw new Error("Editor cancel affordance is not rendered");

    fireEvent.click(cancel);

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
