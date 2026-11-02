import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import "@/shared/i18n";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import TemplateEditor from "./TemplateEditor";

const options = [{ value: "true", label: "Yes" }, { value: "false", label: "No" }];

describe("FormSelect control association", () => {
  it("merges caller descriptions and replaces error with stable help on its actual trigger", () => {
    const field = (error?: string) => <>
      <p id="external">External note</p>
      <FormSelect id="default" label="Default" value="true" onValueChange={vi.fn()} options={options}
        help="Choose a default" error={error} aria-describedby="external external" />
    </>;
    const { rerender } = render(field());
    const trigger = screen.getByRole("combobox", { name: "Default" });
    expect(trigger).toHaveAccessibleDescription("External note Choose a default");
    const helpId = screen.getByText("Choose a default").id;
    rerender(field("Invalid default"));
    expect(trigger).toHaveAccessibleDescription("External note Invalid default");
    expect(trigger).toHaveAttribute("aria-invalid", "true");
    expect(document.getElementById(helpId)).toBeNull();
    const errorId = screen.getByRole("alert").id;
    rerender(field());
    expect(trigger).toHaveAccessibleDescription("External note Choose a default");
    expect(trigger).not.toHaveAttribute("aria-invalid");
    expect(screen.getByText("Choose a default")).toHaveAttribute("id", helpId);
    expect(document.getElementById(errorId)).toBeNull();
    expect(trigger.getAttribute("aria-describedby")?.split(" ")).toEqual(["external", helpId]);
    expect(trigger).toHaveAttribute("id", "default");
  });

  it("preserves explicit invalid state and keyboard selection with focus return", async () => {
    const onValueChange = vi.fn();
    render(<FormSelect label="Default" value="true" onValueChange={onValueChange} options={options}
      error="Invalid default" aria-invalid={false} />);
    const trigger = screen.getByRole("combobox", { name: "Default" });
    expect(trigger).toHaveAttribute("aria-invalid", "false");
    expect(trigger).toHaveAccessibleDescription("Invalid default");
    trigger.focus();
    expect(trigger).toHaveFocus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const yes = await screen.findByRole("option", { name: "Yes" });
    await waitFor(() => expect(yes).toHaveFocus());
    fireEvent.keyDown(yes, { key: "ArrowDown" });
    const no = screen.getByRole("option", { name: "No" });
    await waitFor(() => expect(no).toHaveFocus());
    fireEvent.keyDown(no, { key: "Enter" });
    expect(onValueChange).toHaveBeenCalledWith("false");
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("describes the invalid boolean default in the real template editor", async () => {
    render(<TooltipProvider><TemplateEditor
      initial={{ id: "template", name: "Network", slug: "network", status: "draft", description: "", updatedAt: "",
        variables: [{ key: "enabled", label: "Enabled", value_type: "boolean", default_value: "invalid" }], steps: [] }}
      saving={false} canSave onSave={vi.fn()} onCancel={vi.fn()}
    /></TooltipProvider>);
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));
    const trigger = await screen.findByRole("combobox", { name: "Default" });
    expect(trigger).toHaveAttribute("id", "var-0-default-input");
    expect(trigger).toHaveAttribute("aria-invalid", "true");
    expect(trigger).toHaveAccessibleDescription("enabled: must be true or false");
  });
});
