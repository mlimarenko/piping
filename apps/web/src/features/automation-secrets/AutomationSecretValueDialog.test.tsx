import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import AutomationSecretValueDialog from "./AutomationSecretValueDialog";
import type { AutomationSecret } from "@/features/automation-secrets/api";

const secret = (over: Partial<AutomationSecret> = {}): AutomationSecret => ({
  id: "s1",
  key: "grad-token",
  description: null,
  has_value: false,
  is_masked: true,
  usage_count: 0,
  created_at: "2026-07-01T00:00:00Z",
  ...over,
});

function setup(over: Partial<AutomationSecret> = {}) {
  const onSetValue = vi.fn().mockResolvedValue(undefined);
  const onClearValue = vi.fn().mockResolvedValue(undefined);
  const onOpenChange = vi.fn();
  render(
    <AutomationSecretValueDialog
      secret={secret(over)}
      open
      onOpenChange={onOpenChange}
      onSetValue={onSetValue}
      onClearValue={onClearValue}
    />,
  );
  return { onSetValue, onClearValue, onOpenChange };
}

describe("AutomationSecretValueDialog", () => {
  it("locks the write-only input while a clear request is pending", async () => {
    let settle!: () => void;
    const onClearValue = vi.fn(() => new Promise<void>((resolve) => { settle = resolve; }));
    render(<AutomationSecretValueDialog secret={secret({ has_value: true })} open
      onOpenChange={vi.fn()} onSetValue={vi.fn()} onClearValue={onClearValue} />);
    fireEvent.click(screen.getByRole("button", { name: /clear value/i }));
    expect(screen.getByLabelText(/new value/i)).toBeDisabled();
    expect(screen.getByRole("button", { name: /cancel/i })).toBeDisabled();
    settle();
    await waitFor(() => expect(screen.getByLabelText(/new value/i)).toBeEnabled());
  });
  it("cancels without setting or clearing a value", () => {
    const { onSetValue, onClearValue, onOpenChange } = setup();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(onSetValue).not.toHaveBeenCalled();
    expect(onClearValue).not.toHaveBeenCalled();
  });

  it("starts with an empty input and a disabled save (write-only, never prefilled)", () => {
    setup({ has_value: true });
    expect(screen.getByLabelText(/new value/i)).toHaveValue("");
    expect(screen.getByRole("button", { name: /save value/i })).toBeDisabled();
  });

  it("sets a trimmed value and closes", async () => {
    const { onSetValue, onOpenChange } = setup();

    fireEvent.change(screen.getByLabelText(/new value/i), {
      target: { value: "  s3cr3t  " },
    });
    const save = screen.getByRole("button", { name: /save value/i });
    expect(save).toBeEnabled();
    fireEvent.click(save);

    expect(onSetValue).toHaveBeenCalledWith("s3cr3t");
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("only offers Clear when a value is already stored", () => {
    setup({ has_value: false });
    expect(screen.queryByRole("button", { name: /clear value/i })).not.toBeInTheDocument();
  });

  it("clears the stored value", async () => {
    const { onClearValue, onOpenChange } = setup({ has_value: true });

    fireEvent.click(screen.getByRole("button", { name: /clear value/i }));

    expect(onClearValue).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("keeps a failed clear open, unlocks the input, and permits retry", async () => {
    const onClearValue = vi.fn().mockRejectedValueOnce(new Error("request failed")).mockResolvedValueOnce(undefined);
    const onOpenChange = vi.fn();
    render(<AutomationSecretValueDialog secret={secret({ has_value: true })} open
      onOpenChange={onOpenChange} onSetValue={vi.fn()} onClearValue={onClearValue} />);
    fireEvent.click(screen.getByRole("button", { name: /clear value/i }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByLabelText(/new value/i)).toBeEnabled();
    expect(onOpenChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /clear value/i }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(onClearValue).toHaveBeenCalledTimes(2);
  });
});
