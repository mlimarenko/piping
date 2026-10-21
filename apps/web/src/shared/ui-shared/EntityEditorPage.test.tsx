import { describe, expect, it, vi } from "vitest";
import { screen, fireEvent, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";

describe("EntityEditorPage", () => {
  it("blocks sidebar navigation and reload during saving, retaining discard protection after failure", () => {
    const navigate = vi.fn();
    const props = { title: "Draft", backLabel: "Back", onBack: navigate, dirty: true };
    const view = (saving: boolean) => <><a href="/audit-logs" onClick={navigate}>Sidebar audit</a>
      <EntityEditorPage scope="routed" {...props} saving={saving}><input aria-label="Draft name" defaultValue="Unsaved draft" /></EntityEditorPage></>;
    const { rerender } = render(view(true));
    fireEvent.click(screen.getByRole("link", { name: "Sidebar audit" }));
    expect(navigate).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(navigate).not.toHaveBeenCalled();
    rerender(view(false));
    expect(screen.getByRole("textbox", { name: "Draft name" })).toHaveValue("Unsaved draft");
    fireEvent.click(screen.getByRole("link", { name: "Sidebar audit" }));
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(navigate).not.toHaveBeenCalled();
  });
  it("renders the title, description and breadcrumbs", () => {
    render(
      <EntityEditorPage scope="routed"
        title="router-1"
        description="Devices"
        backLabel="Back"
        onBack={() => {}}
        breadcrumbs={[{ key: "devices", label: "Devices" }, { key: "router-1", label: "router-1" }]}
      >
        <div>body</div>
      </EntityEditorPage>,
    );

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("router-1");
    fireEvent.click(screen.getByRole("button", { name: "Devices" }));
    expect(screen.getByText("Devices", { selector: '[data-state="open"]' })).toBeInTheDocument();
    expect(screen.getByRole("navigation")).toHaveTextContent("Devices");
    expect(screen.getByText("body")).toBeInTheDocument();
  });

  it("calls onBack from the back button and onSave/onCancel from the footer", () => {
    const onBack = vi.fn();
    const onSave = vi.fn();
    const onCancel = vi.fn();
    render(
      <EntityEditorPage scope="routed"
        title="new device"
        backLabel="Back"
        onBack={onBack}
        onSave={onSave}
        onCancel={onCancel}
        saveLabel="Create"
      >
        <div>body</div>
      </EntityEditorPage>,
    );

    fireEvent.click(screen.getByRole("button", { name: /back/i }));
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onBack).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("disables Save while saving and hides it for a read-only page", () => {
    const { rerender } = render(
      <EntityEditorPage scope="routed" title="t" backLabel="Back" onBack={() => {}} onSave={() => {}} saving>
        <div />
      </EntityEditorPage>,
    );
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    rerender(
      <EntityEditorPage scope="routed" title="t" backLabel="Back" onBack={() => {}}>
        <div />
      </EntityEditorPage>,
    );
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  });

  it("shows the unsaved banner and blocks beforeunload while dirty", () => {
    const guard = vi.fn();
    const { unmount } = render(
      <EntityEditorPage scope="routed"
        title="t"
        backLabel="Back"
        onBack={() => {}}
        dirty
        dirtyMessage="You have unsaved changes"
      >
        <div />
      </EntityEditorPage>,
    );

    expect(screen.getByText("You have unsaved changes")).toBeInTheDocument();

    const event = new Event("beforeunload", { cancelable: true });
    event.preventDefault = guard;
    window.dispatchEvent(event);
    expect(guard).toHaveBeenCalled();

    unmount();
    // After unmount the listener is removed: preventDefault must not fire again.
    guard.mockClear();
    window.dispatchEvent(new Event("beforeunload", { cancelable: true }));
    expect(guard).not.toHaveBeenCalled();
  });

  it("routes a dirty Back through the shared discard confirmation", () => {
    const onBack = vi.fn();
    render(
      <EntityEditorPage scope="routed" title="t" backLabel="Back" onBack={onBack} dirty dirtyMessage="dirty">
        <div />
      </EntityEditorPage>,
    );

    fireEvent.click(screen.getByRole("button", { name: /back/i }));
    expect(onBack).not.toHaveBeenCalled();

    const dialog = screen.getByRole("alertdialog");
    // Footer order: continue-editing (cancel) then discard (action).
    fireEvent.click(within(dialog).getAllByRole("button")[1]!);
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("keeps a dirty editor open on Cancel until discarding and disables both actions while pending", () => {
    const onCancel = vi.fn();
    const props = { title: "Device", backLabel: "Back", onBack: vi.fn(), onCancel, onSave: vi.fn(), dirty: true };
    const { rerender } = render(<EntityEditorPage scope="routed" {...props}><input aria-label="Name" defaultValue="Router" /></EntityEditorPage>);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Continue editing" }));
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Router");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    rerender(<EntityEditorPage scope="routed" {...props} saving><input aria-label="Name" defaultValue="Router" /></EntityEditorPage>);
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });
});
