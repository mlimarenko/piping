import { render, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "@/shared/hooks/use-theme";
import { Toaster, toast } from "./sonner";

describe("Sonner Toaster", () => {
  it("renders a close button for app notifications", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(
      <QueryClientProvider client={client}>
        <ThemeProvider><Toaster /></ThemeProvider>
      </QueryClientProvider>,
    );

    toast.success("Connected");

    await waitFor(() => {
      expect(document.querySelector("[data-close-button]")).toBeTruthy();
    });
    view.unmount();
    client.clear();
    toast.dismiss();
  });
});
