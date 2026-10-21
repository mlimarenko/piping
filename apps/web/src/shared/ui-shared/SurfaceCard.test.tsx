import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "./SurfaceCard";

describe("SurfaceCard composition", () => {
  it.each([2, 3, 4] as const)("renders ReactNode titles at heading level %s without leaking a native title", (headingLevel) => {
    render(
      <SurfaceCardHeader
        headingLevel={headingLevel}
        title={<span>Device details</span>}
        aria-label="Device header"
      />,
    );
    expect(screen.getByRole("heading", { level: headingLevel, name: "Device details" })).toBeInTheDocument();
    expect(screen.getByLabelText("Device header")).not.toHaveAttribute("title");
  });

  it("renders a labelled card with a section heading, description, content and working actions", () => {
    const add = vi.fn();
    render(
      <SurfaceCard role="region" aria-labelledby="inventory-title" aria-describedby="inventory-description">
        <SurfaceCardHeader
          title={<span id="inventory-title">Inventory</span>}
          description={<span id="inventory-description">Managed devices</span>}
          actions={<button type="button" onClick={add}>Add device</button>}
        />
        <SurfaceCardContent><p>router-01</p></SurfaceCardContent>
      </SurfaceCard>,
    );
    expect(screen.getByRole("region", { name: "Inventory" })).toHaveAccessibleDescription("Managed devices");
    expect(screen.getByRole("heading", { level: 2, name: "Inventory" })).toBeInTheDocument();
    expect(screen.getByText("router-01")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add device" }));
    expect(add).toHaveBeenCalledTimes(1);
  });

  it("supports nested heading levels and native attributes without inventing content", () => {
    render(
      <SurfaceCard variant="inset" className="custom-card" data-testid="card">
        <SurfaceCardHeader title="Connections" headingLevel={3} className="custom-header" data-testid="header" />
        <SurfaceCardContent className="custom-content" aria-label="Connection list">No connections</SurfaceCardContent>
      </SurfaceCard>,
    );
    expect(screen.getByRole("heading", { level: 3, name: "Connections" })).toBeInTheDocument();
    expect(screen.getByTestId("card")).toHaveClass("custom-card");
    expect(screen.getByTestId("header")).toHaveClass("custom-header");
    expect(screen.getByLabelText("Connection list")).toHaveClass("custom-content");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
