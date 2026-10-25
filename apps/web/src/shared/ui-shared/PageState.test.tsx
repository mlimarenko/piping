import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import PageState from "./PageState";

describe("shared content feedback", () => {
  it("announces pending work without creating another page heading or navigation owner", () => {
    render(<PageState kind="loading" title="Loading devices" />);
    expect(screen.getByRole("status")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("status")).toHaveTextContent("Loading devices");
    expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("keeps the owner's retry action and replaces the error announcement after recovery", () => {
    const retry = vi.fn();
    const { rerender } = render(<PageState kind="error" title="Devices unavailable" description="Try loading again."
      action={<button onClick={retry}>Retry devices</button>} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Devices unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Retry devices" }));
    expect(retry).toHaveBeenCalledOnce();
    rerender(<PageState kind="loading" title="Loading devices" />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveAttribute("aria-busy", "true");
  });

  it("uses the shared empty-state action without claiming an error", () => {
    const create = vi.fn();
    render(<PageState kind="empty" title="No devices" action={<button onClick={create}>Create device</button>} />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create device" }));
    expect(create).toHaveBeenCalledOnce();
  });
});
