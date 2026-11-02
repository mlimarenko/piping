import { useRef, useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Dialog, DialogContent, DialogTitle } from "@/shared/ui-primitives/dialog";
import { Sheet, SheetContent, SheetTitle } from "@/shared/ui-primitives/sheet";
import { useCollectionPreviewFocus } from "./useCollectionPreviewFocus";

type State = "visible" | "removed" | "hidden" | "css-hidden" | "disabled" | "inert" | "unfocusable";
function Fixture({ state = "visible", active = true, sheet = false, handoff = false }: {
  state?: State; active?: boolean; sheet?: boolean; handoff?: boolean;
}) {
  const anchor = useRef<HTMLInputElement>(null);
  const other = useRef<HTMLButtonElement>(null);
  const focus = useCollectionPreviewFocus(anchor, active);
  const [open, setOpen] = useState(false);
  const [otherOpen, setOtherOpen] = useState(false);
  const Root = sheet ? Sheet : Dialog;
  const Content = sheet ? SheetContent : DialogContent;
  const Title = sheet ? SheetTitle : DialogTitle;
  return <>
    <input ref={anchor} aria-label="Collection search" />
    <button ref={other}>Other control</button>
    {state !== "removed" && <div hidden={state === "hidden"} inert={state === "inert"} style={state === "css-hidden" ? { display: "none" } : undefined}>
      <button disabled={state === "disabled"} onClick={event => { focus.capture(state === "unfocusable" ? event.currentTarget.parentElement! : event.currentTarget); setOpen(true); }}>Inspect A</button>
    </div>}
    <button>Inspect B</button>
    <Root open={open} onOpenChange={setOpen}>
      <Content description="Preview" onCloseAutoFocus={event => {
        if (handoff) other.current?.focus();
        focus.onCloseAutoFocus(event);
      }}>
        <Title>Preview A</Title>
        <button onClick={() => setOpen(false)}>Done</button>
        <button onClick={() => { focus.suppress(); setOpen(false); }}>Navigate</button>
        <button onClick={() => { setOtherOpen(true); setOpen(false); }}>Open other preview</button>
      </Content>
    </Root>
    <Dialog open={otherOpen} onOpenChange={setOtherOpen}><DialogContent description="Other preview"><DialogTitle>Other preview</DialogTitle><button>Other preview action</button></DialogContent></Dialog>
  </>;
}

describe.each([false, true])("collection preview focus (sheet=%s)", sheet => {
  it("returns to the exact surviving opener", async () => {
    render(<Fixture sheet={sheet} />);
    const opener = screen.getByRole("button", { name: "Inspect A" });
    fireEvent.click(opener);
    fireEvent.keyDown(await screen.findByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(opener).toHaveFocus());
  });
  it.each<State>(["removed", "hidden", "css-hidden", "disabled", "inert", "unfocusable"])("uses the explicit anchor for %s opener, not another row", async state => {
    const view = render(<Fixture sheet={sheet} state={state === "unfocusable" ? state : "visible"} />);
    fireEvent.click(screen.getByRole("button", { name: "Inspect A" }));
    await screen.findByRole("dialog");
    view.rerender(<Fixture sheet={sheet} state={state} />);
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Collection search" })).toHaveFocus());
  });
  it("does not restore an inactive owner", async () => {
    const view = render(<Fixture sheet={sheet} />);
    fireEvent.click(screen.getByRole("button", { name: "Inspect A" }));
    await screen.findByRole("dialog");
    view.rerender(<Fixture sheet={sheet} active={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("textbox")).not.toHaveFocus();
    expect(screen.getByRole("button", { name: "Inspect A" })).not.toHaveFocus();
  });
  it("respects explicit navigation suppression", async () => {
    render(<Fixture sheet={sheet} />);
    fireEvent.click(screen.getByRole("button", { name: "Inspect A" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Navigate" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Inspect A" })).not.toHaveFocus();
    expect(screen.getByRole("textbox")).not.toHaveFocus();
  });
  it("preserves a meaningful lifecycle handoff", async () => {
    render(<Fixture sheet={sheet} handoff />);
    fireEvent.click(screen.getByRole("button", { name: "Inspect A" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Other control" })).toHaveFocus());
  });
  it("does not steal focus from a competing modal", async () => {
    render(<Fixture sheet={sheet} />);
    fireEvent.click(screen.getByRole("button", { name: "Inspect A" }));
    await screen.findByRole("dialog", { name: "Preview A" });
    fireEvent.click(screen.getByRole("button", { name: "Open other preview" }));
    const otherDialog = await screen.findByRole("dialog", { name: "Other preview" });
    await waitFor(() => expect(otherDialog).toContainElement(document.activeElement as HTMLElement));
    expect(screen.getByRole("textbox", { hidden: true })).not.toHaveFocus();
  });
});
