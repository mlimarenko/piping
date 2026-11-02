import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

vi.mock("@/shared/lib/observability", () => ({ captureException: vi.fn() }));

function Boom({ explode }: { explode: boolean }) {
  if (explode) throw new Error("boom");
  return <div>page content</div>;
}

describe("ErrorBoundary", () => {
  it("renders the fallback instead of the crashed child", () => {
    // React logs the caught error; silence it so the run stays readable.
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    render(
      <ErrorBoundary scope="Page">
        <Boom explode />
      </ErrorBoundary>,
    );
    expect(screen.queryByText("page content")).not.toBeInTheDocument();
    consoleError.mockRestore();
  });

  it("clears the error when resetKey changes, so navigating away recovers", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    function Harness() {
      const [route, setRoute] = useState("/a");
      return (
        <>
          <button type="button" onClick={() => setRoute("/b")}>
            navigate
          </button>
          <ErrorBoundary scope="Page" resetKey={route}>
            <Boom explode={route === "/a"} />
          </ErrorBoundary>
        </>
      );
    }

    render(<Harness />);
    expect(screen.queryByText("page content")).not.toBeInTheDocument();

    // Before resetKey existed the fallback survived navigation, because the
    // only recovery was a retry that re-rendered the very route that threw.
    fireEvent.click(screen.getByRole("button", { name: "navigate" }));
    expect(screen.getByText("page content")).toBeInTheDocument();

    consoleError.mockRestore();
  });
});
