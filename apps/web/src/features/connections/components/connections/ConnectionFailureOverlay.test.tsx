import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import ConnectionFailureOverlay from "@/features/connections/components/connections/ConnectionFailureOverlay";

describe("ConnectionFailureOverlay", () => {
  it("renders a safe localized failure and invokes retry", () => {
    const onRetry = vi.fn();

    render(
      <ConnectionFailureOverlay
        failure={{ kind: "authentication" }}
        onRetry={onRetry}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("Authentication failed");
    expect(screen.queryByText(/backend/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
  });

  it("supports compact section presentation", () => {
    render(
      <ConnectionFailureOverlay
        failure={{ kind: "unsupported" }}
        presentation="section"
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent("Not supported by this device");
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
