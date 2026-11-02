import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import McpAdminPage from "./McpAdminPage";
import { notify } from "@/shared/lib/notifications";
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

describe("McpAdminPage", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.clearAllMocks(); });
  it("links onboarding to token creation and exposes one page heading", () => {
    render(<MemoryRouter><McpAdminPage /></MemoryRouter>);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("link", { name: /create api token/i })).toHaveAttribute(
      "href", "/user/settings?create=token",
    );
  });
  it("does not report copy success when the clipboard rejects", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("Clipboard denied"));
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    render(<MemoryRouter><McpAdminPage /></MemoryRouter>);
    fireEvent.click(screen.getAllByRole("button", { name: /copy/i })[0]!);
    await waitFor(() => expect(notify).toHaveBeenCalledWith("error", expect.any(String)));
    expect(notify).not.toHaveBeenCalledWith("success", expect.anything());
  });
});
