import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import InspectorPane from "./InspectorPane";

describe("target details container", () => {
  it("scopes Escape to inline details and respects a nested prevented Escape", () => {
    const onClose = vi.fn();
    render(<><button>Outside</button><InspectorPane width={320} onResize={vi.fn()} overlay={false} onClose={onClose} title="Router details">
      <h2>Router</h2><button onKeyDown={event => event.preventDefault()}>Nested action</button><button>Card action</button>
    </InspectorPane></>);
    const pane = screen.getByRole("complementary", { name: "Router details" });
    expect(pane).toHaveFocus();
    expect(within(pane).getAllByRole("heading")).toHaveLength(1);
    fireEvent.keyDown(screen.getByRole("button", { name: "Outside" }), { key: "Escape" });
    fireEvent.keyDown(screen.getByRole("button", { name: "Nested action" }), { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("button", { name: "Card action" }), { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it.each([false, true])("uses a dismissible sheet with the card header only (mobile=%s)", mobile => {
    const onClose = vi.fn();
    render(<InspectorPane width={320} onResize={vi.fn()} overlay mobile={mobile} onClose={onClose} title="Router details">
      <h2>Router</h2><button onClick={onClose}>Close card</button>
    </InspectorPane>);
    const dialog = screen.getByRole("dialog", { name: "Router details" });
    expect(within(dialog).getByRole("heading", { name: "Router" })).toBeVisible();
    expect(within(dialog).getAllByRole("button")).toHaveLength(1);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });
});
