import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { expect, it, vi } from "vitest";
import AppShell from "./AppShell";

const mocks = vi.hoisted(() => ({ me: vi.fn() }));
vi.mock("@/features/auth/api", () => ({ me: mocks.me }));
vi.mock("@/app/BrandingContext", () => ({ useBranding: () => ({ appName: "Piping" }) }));
vi.mock("@/shared/lib/build-version", () => ({ BUILD_VERSION_LABEL: "test" }));

it("shows a safe retry state rather than a blank shell on network failure", async () => {
  mocks.me.mockRejectedValue(new TypeError("Private network diagnostic"));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><MemoryRouter><AppShell /></MemoryRouter></QueryClientProvider>);
  expect(await screen.findByRole("alert")).toBeInTheDocument();
  expect(screen.queryByText("Private network diagnostic")).toBeNull();
  fireEvent.click(screen.getByRole("button"));
  await waitFor(() => expect(mocks.me).toHaveBeenCalledTimes(2));
});
