import { render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { describe, expect, it } from "vitest";

import AlertBadge from "./AlertBadge";
import { alertSeverityStatus, rollupHostAlerts } from "./alertSeverity";
import type { HostAlert, HostLabelMetric } from "./api";

function firingAlert(over: Partial<HostAlert>): HostAlert {
  return { name: "alert", severity: "warning", since: null, summary: null, ...over };
}

// A single-series Prometheus instant-query envelope, the shape the list-labels
// cache stores for each metric.
function promValue(raw: string): unknown {
  return {
    status: "success",
    data: { resultType: "vector", result: [{ metric: {}, value: [1700000000, raw] }] },
  };
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

describe("alertSeverityStatus", () => {
  it("maps known severities to their status bucket", () => {
    expect(alertSeverityStatus("critical")).toBe("critical");
    expect(alertSeverityStatus("disaster")).toBe("critical");
    expect(alertSeverityStatus("warning")).toBe("warning");
    expect(alertSeverityStatus("average")).toBe("warning");
    expect(alertSeverityStatus("info")).toBe("normal");
  });

  it("defaults an unknown or empty severity to warning (a firing alert is at least a warning)", () => {
    expect(alertSeverityStatus("totally-made-up")).toBe("warning");
    expect(alertSeverityStatus("")).toBe("warning");
    expect(alertSeverityStatus(null)).toBe("warning");
  });
});

describe("rollupHostAlerts", () => {
  it("returns null status and zero count when there are no alerts and metrics are healthy", () => {
    const rollup = rollupHostAlerts(
      [],
      [metric({ query: "cpu", value: promValue("10"), value_type: "percent", unit: "%" })],
      false,
    );
    expect(rollup.status).toBeNull();
    expect(rollup.count).toBe(0);
  });

  it("does not surface a pure no-data metric rollup (unknown is not a breach)", () => {
    const empty = {
      status: "success",
      data: { resultType: "vector", result: [] },
    } as unknown;
    const rollup = rollupHostAlerts(
      [],
      [metric({ query: "cpu", value: empty, value_type: "percent", unit: "%" })],
      false,
    );
    expect(rollup.status).toBeNull();
  });

  it("picks the worst severity across multiple alerts", () => {
    const rollup = rollupHostAlerts(
      [firingAlert({ severity: "info" }), firingAlert({ severity: "warning" }), firingAlert({ severity: "critical" })],
      [],
      false,
    );
    expect(rollup.status).toBe("critical");
    expect(rollup.count).toBe(3);
  });

  it("escalates to the metric threshold breach when it is worse than the alerts", () => {
    const rollup = rollupHostAlerts(
      [firingAlert({ severity: "info" })],
      // 95% crosses the default percent critical threshold (>= 90).
      [metric({ query: "cpu", value: promValue("95"), value_type: "percent", unit: "%" })],
      false,
    );
    expect(rollup.status).toBe("critical");
    expect(rollup.count).toBe(1);
  });

  it("surfaces a threshold breach with a worst-severity status even when no alerts fire", () => {
    const rollup = rollupHostAlerts(
      [],
      [metric({ query: "cpu", value: promValue("95"), value_type: "percent", unit: "%" })],
      false,
    );
    expect(rollup.status).toBe("critical");
    expect(rollup.count).toBe(0);
  });

  it("does not light a green badge for an info-only firing alert", () => {
    // A green 'alert' pill reads as contradictory; the worst-severity 'normal'
    // is suppressed to null so no badge renders (the alert stays in the hover).
    const rollup = rollupHostAlerts([firingAlert({ severity: "info" })], [], false);
    expect(rollup.status).toBeNull();
    expect(rollup.count).toBe(1);
  });
});

describe("AlertBadge", () => {
  it("renders nothing when there are no alerts and the rollup is healthy", () => {
    const { container } = render(<AlertBadge alerts={[]} metrics={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it("renders the alert count colored by the worst severity", () => {
    const { container } = render(
      <AlertBadge alerts={[firingAlert({ severity: "warning" }), firingAlert({ severity: "critical" })]} />,
    );
    expect(screen.getByText("2")).toBeInTheDocument();
    expect(container.querySelector(".text-status-negative")).not.toBeNull();
  });

  it("renders a severity dot without a count when only a metric threshold breaches", () => {
    const { container } = render(
      <AlertBadge
        alerts={[]}
        metrics={[metric({ query: "cpu", value: promValue("95"), value_type: "percent", unit: "%" })]}
      />,
    );
    // The badge is present (critical tone) but shows no numeric count.
    expect(container.firstChild).not.toBeNull();
    expect(container.querySelector(".text-status-negative")).not.toBeNull();
    expect(screen.queryByText("0")).toBeNull();
  });
});
