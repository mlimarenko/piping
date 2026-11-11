import { useEffect, useState } from "react";
import { fireEvent, screen, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import EntityEditorHost from "./EntityEditorHost";
import { TogglePill } from "./TogglePill";
import PageChrome from "./PageChrome";
import { usePageActionsBlocker } from "./pageChromeContext";

describe("embedded entity editor", () => {
  it("restores a global invoker only after its host committed the inert release", () => {
    function Fixture() {
      const [open, setOpen] = useState(false);
      return <main id="main-content"><PageChrome title="Workspace" actions={<button onClick={() => setOpen(true)}>Create</button>} /><div>Source</div>
        <EntityEditorHost open={open} onOpenChange={setOpen} title="Create object" footer={<button onClick={() => setOpen(false)}>Cancel</button>}><input aria-label="Name" /></EntityEditorHost>
      </main>;
    }
    render(<Fixture />);
    const invoker = screen.getByRole("button", { name: "Create" });
    const focus = vi.spyOn(invoker, "focus");
    invoker.focus(); fireEvent.click(invoker); focus.mockClear();
    focus.mockImplementation(() => { expect(invoker.closest("[inert]")).toBeNull(); });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(focus).toHaveBeenCalledTimes(1);
  });
  it("keeps another owner's blocker and postpones global focus until the last editor releases", () => {
    function Blocker({ active }: { active: boolean }) { usePageActionsBlocker("second-editor", active); return null; }
    function Fixture() {
      const [open, setOpen] = useState(false);
      const [second, setSecond] = useState(false);
      return <><Blocker active={second} /><button onClick={() => setSecond(false)}>Close second editor</button><main id="main-content">
        <PageChrome title="Workspace" actions={<button onClick={() => setOpen(true)}>Create</button>} /><div>Source</div>
        <EntityEditorHost open={open} onOpenChange={setOpen} title="Create object" footer={<button onClick={() => { setSecond(true); setOpen(false); }}>Switch editor</button>}><input aria-label="Name" /></EntityEditorHost>
      </main></>;
    }
    render(<Fixture />);
    const invoker = screen.getByRole("button", { name: "Create" });
    invoker.focus(); fireEvent.click(invoker);
    const focus = vi.spyOn(invoker, "focus");
    fireEvent.click(screen.getByRole("button", { name: "Switch editor" }));
    expect(invoker.closest("[inert]")).not.toBeNull();
    expect(focus).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close second editor" }));
    expect(invoker.closest("[inert]")).toBeNull();
    expect(focus).toHaveBeenCalledTimes(1);
    expect(invoker).toHaveFocus();
  });
  it("returns to its still-mounted source and confirms a changed draft before cancelling", () => {
    const mount = vi.fn(), unmount = vi.fn(), save = vi.fn();
    function Source() {
      useEffect(() => { mount(); return () => unmount(); }, []);
      return <input aria-label="Session buffer" defaultValue="retained output" />;
    }
    function Fixture() {
      const [open, setOpen] = useState(false);
      return <main id="main-content"><PageChrome title="Workspace" actions={<button>Page action</button>} /><div><Source /><button onClick={() => setOpen(true)}>Edit</button></div>
        <EntityEditorHost open={open} onOpenChange={setOpen} title="Edit object" footer={<>
          <button data-editor-cancel onClick={() => setOpen(false)}>Cancel</button><button onClick={save}>Save</button>
        </>}><input autoFocus aria-label="Object name" defaultValue="old name" /></EntityEditorHost>
      </main>;
    }
    render(<Fixture />);
    const session = screen.getByRole("textbox", { name: "Session buffer" });
    const invoker = screen.getByRole("button", { name: "Edit" });
    invoker.focus(); fireEvent.click(invoker);
    expect(screen.getByRole("heading", { name: "Edit object" })).toHaveFocus();
    expect(screen.getByRole("heading", { name: "Edit object", level: 2 })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Workspace");
    expect(screen.getByText("Page action").closest("[inert]")).toBeInTheDocument();
    expect(session.closest("[inert]")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Object name" }), { target: { value: "new name" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    const confirmation = screen.getByRole("alertdialog");
    fireEvent.click(within(confirmation).getByRole("button", { name: "Continue editing" }));
    expect(screen.getByRole("textbox", { name: "Object name" })).toHaveValue("new name");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard" }));
    expect(screen.queryByRole("heading", { name: "Edit object" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Session buffer" })).toBe(session);
    expect(invoker).toHaveFocus();
    expect(screen.getByRole("button", { name: "Page action" }).closest("[inert]")).toBeNull();
    expect(save).not.toHaveBeenCalled();
    expect(mount).toHaveBeenCalledTimes(1);
    expect(unmount).not.toHaveBeenCalled();
  });

  it("treats an aria-pressed toggle as a changed draft", () => {
    function Fixture() {
      const [open, setOpen] = useState(false);
      const [active, setActive] = useState(false);
      return <main id="main-content"><button onClick={() => setOpen(true)}>Edit</button>
        <EntityEditorHost open={open} onOpenChange={setOpen} title="Edit object" footer={
          <button data-editor-cancel onClick={() => setOpen(false)}>Cancel</button>
        }>
          <TogglePill active={active} onClick={() => setActive(value => !value)}>Enabled</TogglePill>
        </EntityEditorHost>
      </main>;
    }
    render(<Fixture />);

    // An untouched toggle closes without a prompt.
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("heading", { name: "Edit object" })).not.toBeInTheDocument();

    // Flipping the aria-pressed toggle marks the draft changed.
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Enabled" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });
});
