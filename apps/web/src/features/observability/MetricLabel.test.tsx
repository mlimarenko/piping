import { render, screen } from "@testing-library/react";
import "@/shared/i18n";
import i18n from "@/shared/i18n";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { HostAvailabilityIndicator } from "./HostAvailabilityIndicator";
import { resolveHostAvailabilityState } from "./hostAvailability";
import MetricLabel, { HostLabelStrip } from "./MetricLabel";
import type { HostLabelEntry, HostLabelMetric } from "./api";

// A single-series Prometheus instant-query envelope, the exact shape the
// list-labels cache stores and the inspector live path returns.
function promValue(raw: string): unknown {
  return {
    status: "success",
    data: { resultType: "vector", result: [{ metric: {}, value: [1700000000, raw] }] },
  };
}

function promEmpty(): unknown {
  return { status: "success", data: { resultType: "vector", result: [] } };
}

function metric(over: Partial<HostLabelMetric>): HostLabelMetric {
  return {
    query: "q",
    value: promValue("1"),
    label: "Metric",
    unit: null,
    value_type: null,
    widget_type: null,
    list_render: "value",
    options: {},
    ...over,
  };
}

function hostEntry(over: Partial<HostLabelEntry> = {}): HostLabelEntry {
  return {
    host_id: "host-1",
    stale: false,
    metrics: [],
    alerts: [],
    ...over,
  };
}

describe("MetricLabel", () => {
  it("renders a value pill and colors a threshold-crossing percent critical", () => {
    const { container } = render(
      <MetricLabel
        metric={metric({
          label: "CPU",
          value: promValue("95"),
          unit: "%",
          value_type: "percent",
          list_render: "value",
        })}
      />,
    );
    expect(screen.getByText(/95/)).toBeInTheDocument();
    // 95% crosses the default percent critical threshold (>= 90) → negative tone.
    expect(container.querySelector(".text-status-negative")).not.toBeNull();
  });

  it("renders dot mode without a visible value but keeps an accessible summary", () => {
    render(
      <MetricLabel
        metric={metric({ label: "Up", value: promValue("1"), value_type: "state", list_render: "dot" })}
      />,
    );
    expect(screen.getByLabelText(/Up:/)).toBeInTheDocument();
  });

  it("shows an em dash when the metric has no value", () => {
    render(
      <MetricLabel
        metric={metric({ label: "CPU", value: promEmpty(), value_type: "percent", list_render: "value" })}
      />,
    );
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("caps visible labels and renders an overflow chip", () => {
    const metrics = [0, 1, 2, 3].map((i) =>
      metric({ query: `q${i}`, label: `M${i}`, value: promValue(`${i}`), list_render: "value" }),
    );
    render(<HostLabelStrip metrics={metrics} max={3} />);
    expect(screen.getByText("+1")).toBeInTheDocument();
  });

  it("renders nothing for an empty or missing metrics list", () => {
    const { container } = render(<HostLabelStrip metrics={[]} />);
    expect(container.firstChild).toBeNull();
  });
});

describe("HostAvailabilityIndicator", () => {
  it("uses the backend semantic metric as the availability source", () => {
    const availability = metric({
      label: "Reachable",
      value: promValue("1"),
      value_type: "state",
      semantic: "availability",
    });
    expect(resolveHostAvailabilityState(hostEntry({ metrics: [metric({ query: "cpu" }), availability] }))).toBe("up");

    render(<HostAvailabilityIndicator entry={hostEntry({ metrics: [availability] })} />);
    expect(screen.getByLabelText(/Up: Reachable/)).toBeInTheDocument();
  });

  it("reports down, stale and unknown without query-name heuristics", () => {
    const down = hostEntry({
      metrics: [metric({ label: "Up", value: promValue("0"), value_type: "state", semantic: "availability" })],
    });
    const stale = hostEntry({ stale: true, metrics: down.metrics });

    expect(resolveHostAvailabilityState(down)).toBe("down");
    expect(resolveHostAvailabilityState(stale)).toBe("stale");
    expect(
      resolveHostAvailabilityState(hostEntry({ metrics: [metric({ query: "up", value: promValue("1") })] })),
    ).toBe("unknown");
  });
});

describe("HostAvailabilityIndicator localization", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("ru");
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("renders Russian labels for every availability state, not the English call-site fallback", () => {
    const up = hostEntry({
      metrics: [metric({ label: "Reachable", value: promValue("1"), value_type: "state", semantic: "availability" })],
    });
    const { rerender } = render(<HostAvailabilityIndicator entry={up} />);
    expect(screen.getByLabelText(/^Доступен/)).toBeInTheDocument();
    expect(screen.queryByLabelText(/Up: Reachable/)).not.toBeInTheDocument();

    const down = hostEntry({
      metrics: [metric({ label: "Up", value: promValue("0"), value_type: "state", semantic: "availability" })],
    });
    rerender(<HostAvailabilityIndicator entry={down} />);
    expect(screen.getByLabelText(/^Недоступен/)).toBeInTheDocument();

    rerender(<HostAvailabilityIndicator entry={hostEntry({ stale: true, metrics: down.metrics })} />);
    expect(screen.getByLabelText(/^Устарело/)).toBeInTheDocument();

    rerender(<HostAvailabilityIndicator entry={hostEntry()} />);
    expect(screen.getByLabelText(/^Неизвестно/)).toBeInTheDocument();
  });
});
