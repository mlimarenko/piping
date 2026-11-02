import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import "@/shared/i18n";
import i18n from "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { getSessionHealth, type ConnectionSessionHealth } from "@/features/connections/api";
import SessionHealthBar from "./SessionHealthBar";

vi.mock("@/features/connections/api", () => ({
  getSessionHealth: vi.fn(),
}));

const mockedGetSessionHealth = vi.mocked(getSessionHealth);

function sampleHealth(): ConnectionSessionHealth {
  return {
    session_id: "session-1",
    connection_type: "ssh",
    route_mode: "proxied",
    state: "ok",
    reason_code: null,
    worst_leg_id: null,
    proxy_node_name: "grad",
    updated_at: new Date().toISOString(),
    stale_after_ms: 7_500,
    legs: [
      {
        id: "client_backend",
        state: "ok",
        latency_ms: 12,
        rx_bps: null,
        tx_bps: null,
        rx_bytes: 156,
        tx_bytes: 1_700,
        quality_label: null,
        quality_score: null,
        message: null,
      },
      {
        id: "backend_proxy",
        state: "ok",
        latency_ms: 37,
        rx_bps: 100,
        tx_bps: 200,
        rx_bytes: 1_600,
        tx_bytes: 0,
        quality_label: "good",
        quality_score: 100,
        message: null,
      },
      {
        id: "proxy_target",
        state: "ok",
        latency_ms: 76,
        rx_bps: 300,
        tx_bps: 400,
        rx_bytes: 1_600,
        tx_bytes: 0,
        quality_label: "warning",
        quality_score: 75,
        message: null,
      },
    ],
  };
}

function renderHealthBar() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <SessionHealthBar sessionId="session-1" viewerStatus="connected" />
    </QueryClientProvider>,
  );
}

describe("SessionHealthBar rendering", () => {
  beforeEach(async () => {
    mockedGetSessionHealth.mockReset();
    mockedGetSessionHealth.mockResolvedValue(sampleHealth());
    await i18n.changeLanguage("ru");
  });

  it("renders a localized proxy instance label in the route map", async () => {
    renderHealthBar();

    const bar = await screen.findByRole("button", {
      name: /Вы - Piping - grad прокси - хост/,
    });

    expect(bar).toHaveAccessibleName(/Вы - Piping - grad прокси - хост/);
    expect(bar).toHaveAccessibleName(/Piping - grad прокси: Хорошо/);
    expect(bar).toHaveAccessibleName(/grad прокси - хост: Хорошо/);
  });

  it("opens the health details from the compact trigger and dismisses with Escape", async () => {
    renderHealthBar();
    const trigger = await screen.findByRole("button", { name: /Вы - Piping - grad прокси - хост/ });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(trigger);
    expect(screen.getByRole("dialog")).toBeVisible();
    expect(screen.getByRole("img", { name: /Вы - Piping - grad прокси - хост/ })).toBeVisible();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
