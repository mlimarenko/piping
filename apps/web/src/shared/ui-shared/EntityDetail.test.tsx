import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { Database, Pencil, Trash2 } from "lucide-react";
import EntityDetail, { type EntityDetailProps } from "./EntityDetail";

const baseProps: EntityDetailProps = {
  icon: Database,
  title: "VictoriaMetrics",
  description: "vm",
  backHref: "/observability?tab=inspector-metrics&view=sources",
  fields: [{ label: "Slug", value: "vm" }],
  actions: [],
};

function renderDetail(overrides: Partial<EntityDetailProps> = {}) {
  return render(
    <MemoryRouter>
      <EntityDetail {...baseProps} {...overrides} />
    </MemoryRouter>,
  );
}

describe("EntityDetail pure frame", () => {
  it("cancels with Escape and returns focus without confirming", async () => {
    const onConfirm = vi.fn();
    renderDetail({ actions: [{ key: "delete", label: "Delete", allowed: true, confirm: { title: "Delete", description: "Delete A?", onConfirm } }] });
    const trigger = screen.getByRole("button", { name: "Delete" });
    fireEvent.click(trigger);
    fireEvent.keyDown(await screen.findByRole("alertdialog"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("uses current permission rather than the permission captured when opened", async () => {
    const onConfirm = vi.fn();
    const actions = (allowed: boolean): EntityDetailProps["actions"] => [{ key: "delete", label: "Delete", allowed, confirm: { title: "Delete", description: "Delete A?", onConfirm } }];
    const view = renderDetail({ actions: actions(true) });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    view.rerender(<MemoryRouter><EntityDetail {...baseProps} actions={actions(false)} /></MemoryRouter>);
    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();
  });
  it("renders the entity title, fields and status chip", () => {
    renderDetail({
      status: { tone: "positive", label: "Enabled" },
      fields: [{ label: "Slug", value: "vm" }],
    });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("VictoriaMetrics");
    expect(screen.getByText("Slug")).toBeInTheDocument();
    expect(screen.getByText("vm")).toBeInTheDocument();
    expect(screen.getByText("Enabled")).toBeInTheDocument();
  });

  it("renders an empty field value as an em dash", () => {
    renderDetail({ fields: [{ label: "Credential reference", value: "" }] });
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("renders allowed actions as a link and a button", () => {
    renderDetail({
      actions: [
        { key: "edit", label: "Edit", icon: Pencil, href: "/observability/sources/vm/edit", allowed: true },
        { key: "delete", label: "Delete", icon: Trash2, variant: "outline", allowed: true },
      ],
    });
    expect(screen.getByRole("link", { name: "Edit" })).toHaveAttribute(
      "href",
      "/observability/sources/vm/edit",
    );
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });

  it("does not render disallowed actions", () => {
    renderDetail({
      actions: [
        { key: "edit", label: "Edit", href: "/observability/sources/vm/edit", allowed: false },
        { key: "delete", label: "Delete", allowed: false },
      ],
    });
    expect(screen.queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("does not invoke onConfirm when the destructive action is cancelled", async () => {
    const onConfirm = vi.fn();
    renderDetail({
      actions: [
        {
          key: "delete",
          label: "Delete",
          variant: "outline",
          allowed: true,
          confirm: { title: "Delete", description: 'Delete "VictoriaMetrics"?', onConfirm },
        },
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText('Delete "VictoriaMetrics"?')).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("invokes onConfirm when the destructive action is accepted", async () => {
    const onConfirm = vi.fn();
    renderDetail({
      actions: [
        {
          key: "delete",
          label: "Delete",
          variant: "outline",
          allowed: true,
          confirm: { title: "Delete", description: 'Delete "VictoriaMetrics"?', onConfirm },
        },
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("disables the confirmation action while pending", async () => {
    const onConfirm = vi.fn();
    const confirm = { title: "Delete", description: "Delete?", onConfirm };
    const actions = (disabled: boolean): EntityDetailProps["actions"] => [
      { key: "delete", label: "Delete", variant: "outline", allowed: true, disabled, confirm },
    ];
    const view = renderDetail({ actions: actions(false) });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByRole("button", { name: "Delete" })).not.toBeDisabled();

    view.rerender(
      <MemoryRouter>
        <EntityDetail {...baseProps} actions={actions(true)} />
      </MemoryRouter>,
    );
    expect(within(dialog).getByRole("button", { name: "Delete" })).toBeDisabled();
  });

  it("returns focus to the delete trigger after cancelling", async () => {
    const onConfirm = vi.fn();
    renderDetail({
      actions: [
        {
          key: "delete",
          label: "Delete",
          variant: "outline",
          allowed: true,
          confirm: { title: "Delete", description: "Delete?", onConfirm },
        },
      ],
    });
    const trigger = screen.getByRole("button", { name: "Delete" });
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("keeps the confirmed target snapshot when the same key is retargeted", async () => {
    const onConfirmA = vi.fn();
    const onConfirmB = vi.fn();
    const actions = (target: "A" | "B", disabled = false): EntityDetailProps["actions"] => [
      {
        key: "delete",
        label: "Delete",
        variant: "outline",
        allowed: true,
        disabled,
        confirm: {
          title: "Delete",
          description: target === "A" ? 'Delete "A"?' : 'Delete "B"?',
          onConfirm: target === "A" ? onConfirmA : onConfirmB,
        },
      },
    ];
    const view = renderDetail({ actions: actions("A") });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText('Delete "A"?')).toBeInTheDocument();

    // Same key retargeted to B while pending: the dialog still shows A and the
    // confirmation button is disabled by the current pending flag.
    view.rerender(
      <MemoryRouter>
        <EntityDetail {...baseProps} actions={actions("B", true)} />
      </MemoryRouter>,
    );
    expect(within(dialog).getByText('Delete "A"?')).toBeInTheDocument();
    expect(within(dialog).queryByText('Delete "B"?')).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Delete" })).toBeDisabled();

    // Same key retargeted to B, no longer pending: confirm still invokes A, not B.
    view.rerender(
      <MemoryRouter>
        <EntityDetail {...baseProps} actions={actions("B")} />
      </MemoryRouter>,
    );
    const confirmButton = within(dialog).getByRole("button", { name: "Delete" });
    expect(confirmButton).not.toBeDisabled();
    fireEvent.click(confirmButton);
    expect(onConfirmA).toHaveBeenCalledOnce();
    expect(onConfirmB).not.toHaveBeenCalled();
  });

  it("renders the delete error as an alert", () => {
    renderDetail({ error: "Failed to delete" });
    expect(screen.getByRole("alert")).toHaveTextContent("Failed to delete");
  });

  it("renders children under the field grid", () => {
    renderDetail({ children: <div>metrics in this set</div> });
    expect(screen.getByText("metrics in this set")).toBeInTheDocument();
  });

  it("renders a long value in full so it can wrap rather than clip", () => {
    const long = "x".repeat(300);
    renderDetail({ fields: [{ label: "Query", value: <span>{long}</span> }] });
    expect(screen.getByText(long)).toBeInTheDocument();
  });
});
