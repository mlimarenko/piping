import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  isTelemetryAllowed,
  loadTelemetryPolicy,
  resetTelemetryConsentForTests,
  setTelemetryConsent,
  telemetryConsent,
} from "./consent";
import { boundedStack, routeSegment } from "./events";
import {
  flush,
  resetTelemetryForTests,
  startTelemetry,
  trackApiFailure,
  trackProductEvent,
} from "./rum";
import TelemetryConsentBanner from "@/app/layout/TelemetryConsentBanner";
import TelemetryConsentSetting from "@/features/user-settings/TelemetryConsentSetting";
import { initTelemetry } from "./index";

const fetchMock = vi.fn<(input: RequestInfo | URL, init?: RequestInit) => Promise<Response>>(
  () => Promise.resolve(new Response(null, { status: 202 })),
);

function policyResponse(enabled: boolean, defaultEnabled: boolean): Response {
  return Response.json({ enabled, default_enabled: defaultEnabled });
}

function reportBody(): unknown {
  const call = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
  const body = call[1].body;
  if (typeof body !== "string") throw new Error("report body is not a string");
  return JSON.parse(body);
}

beforeEach(async () => {
  window.localStorage.clear();
  resetTelemetryForTests();
  resetTelemetryConsentForTests();
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockResolvedValueOnce(policyResponse(true, false));
  await loadTelemetryPolicy();
  fetchMock.mockClear();
  fetchMock.mockResolvedValue(new Response(null, { status: 202 }));
});

afterEach(() => {
  resetTelemetryForTests();
  vi.unstubAllGlobals();
});

describe("telemetry consent", () => {
  it("starts unset and stays inert without consent", () => {
    expect(telemetryConsent()).toBe("unset");
    expect(isTelemetryAllowed()).toBe(false);

    startTelemetry();
    trackProductEvent("app_opened");
    trackApiFailure("/api/v1/connections/sessions?token=secret", 503);
    flush();

    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("sends nothing after an explicit denial", () => {
    setTelemetryConsent("denied");
    startTelemetry();
    trackProductEvent("playbook_run_started");
    flush();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("sends only the bounded vocabulary after consent", () => {
    setTelemetryConsent("granted");
    startTelemetry();
    trackProductEvent("connection_session_started");
    trackApiFailure("/api/v1/connections/sessions?token=secret", 503);
    trackApiFailure("/api/v1/connections", 404);
    flush();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const body = reportBody() as { version: number; events: unknown[] };
    expect(body.version).toBe(1);
    expect(body.events).toEqual([
      { type: "product_event", event: "app_opened" },
      { type: "product_event", event: "connection_session_started" },
      { type: "api_failure", route: "connections", status: 503 },
    ]);
    expect(JSON.stringify(body)).not.toContain("secret");
    expect(JSON.stringify(body)).not.toContain("/api/v1/connections");
  });

  it("clears unsent data when consent is withdrawn", () => {
    setTelemetryConsent("granted");
    startTelemetry();
    trackProductEvent("playbook_run_started");
    setTelemetryConsent("denied");
    flush();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("bounded report fields", () => {
  it("keeps one path segment", () => {
    expect(routeSegment("/connections/host-1?token=secret")).toBe("connections");
    expect(routeSegment("/api/v1/playbooks/runs/123")).toBe("playbooks");
    expect(routeSegment("/")).toBe("other");
    expect(routeSegment("")).toBe("other");
  });

  it("drops a segment outside the shared allow-list", () => {
    expect(routeSegment("/tenant-550e8400e29b41d4/inventory")).toBe("other");
    expect(routeSegment("https://evil.example/connections")).toBe("other");
  });

  it("caps a stack before it leaves the tab", () => {
    const stack = "x".repeat(5_000);
    expect(boundedStack(stack)?.length).toBe(4_000);
    expect(boundedStack(undefined)).toBeUndefined();
    expect(boundedStack("   ")).toBeUndefined();
  });
});

describe("consent banner", () => {
  it("shows a first-run choice and records a denial", () => {
    render(<TelemetryConsentBanner />);
    expect(screen.getByRole("region", { name: /usage data/i })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /not now/i }));
    expect(telemetryConsent()).toBe("denied");
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
  });

  it("records an allowance", async () => {
    render(<TelemetryConsentBanner />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /^allow$/i }));
      await Promise.resolve();
    });
    expect(telemetryConsent()).toBe("granted");
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
  });
});

describe("deployment policy", () => {
  it("starts default-on users without writing a choice and hides the banner", async () => {
    resetTelemetryConsentForTests();
    fetchMock.mockResolvedValueOnce(policyResponse(true, true));
    render(<><TelemetryConsentBanner /><TelemetryConsentSetting /></>);
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
    await act(async () => { initTelemetry(); await loadTelemetryPolicy(); });
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
    expect(telemetryConsent()).toBe("unset");
    expect(window.localStorage.getItem("piping.telemetry.consent")).toBeNull();
    expect(screen.getByText("Allowed")).toBeInTheDocument();
    await waitFor(() => {
      flush();
      expect(fetchMock).toHaveBeenCalledWith("/api/v1/rum/report", expect.objectContaining({ method: "POST" }));
    });
    expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/config")).toHaveLength(1);
  });

  it("keeps a saved denial despite a default-on policy", async () => {
    window.localStorage.setItem("piping.telemetry.consent", "denied");
    resetTelemetryConsentForTests();
    fetchMock.mockResolvedValueOnce(policyResponse(true, true));
    await act(async () => { initTelemetry(); await loadTelemetryPolicy(); });
    render(<TelemetryConsentBanner />);
    expect(telemetryConsent()).toBe("denied");
    expect(isTelemetryAllowed()).toBe(false);
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
    expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/report")).toHaveLength(0);
  });

  it("overrides a stored grant when deployment disables telemetry", async () => {
    window.localStorage.setItem("piping.telemetry.consent", "granted");
    resetTelemetryConsentForTests();
    fetchMock.mockResolvedValueOnce(policyResponse(false, true));
    await act(async () => { initTelemetry(); await loadTelemetryPolicy(); });
    render(<><TelemetryConsentBanner /><TelemetryConsentSetting /></>);
    expect(telemetryConsent()).toBe("granted");
    expect(isTelemetryAllowed()).toBe(false);
    expect(screen.queryByRole("region", { name: /usage data/i })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /allow/i })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /turn off/i }));
    expect(telemetryConsent()).toBe("denied");
    expect(screen.getByRole("button", { name: /turn off/i })).toBeDisabled();
    expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/report")).toHaveLength(0);
  });

  it.each([
    ["network failure", () => Promise.reject(new Error("offline"))],
    ["invalid response", () => Promise.resolve(Response.json({ enabled: true }))],
    ["server error", () => Promise.resolve(new Response(null, { status: 503 }))],
  ])("keeps collection off on %s", async (_label, response) => {
    window.localStorage.setItem("piping.telemetry.consent", "granted");
    resetTelemetryConsentForTests();
    fetchMock.mockImplementationOnce(response);
    await act(async () => { initTelemetry(); await loadTelemetryPolicy(); });
    expect(isTelemetryAllowed()).toBe(false);
    expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/report")).toHaveLength(0);
  });

  it("lets default-on users opt out and discards pending reports", async () => {
    resetTelemetryConsentForTests();
    fetchMock.mockResolvedValueOnce(policyResponse(true, true));
    await act(async () => { initTelemetry(); await loadTelemetryPolicy(); });
    render(<TelemetryConsentSetting />);
    startTelemetry();
    trackProductEvent("playbook_run_started");
    fireEvent.click(screen.getByRole("button", { name: /turn off/i }));
    flush();
    expect(telemetryConsent()).toBe("denied");
    expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/report")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: /allow usage data/i }));
    await waitFor(() => {
      flush();
      expect(fetchMock.mock.calls.filter(([url]) => url === "/api/v1/rum/report")).toHaveLength(1);
    });
    const report = fetchMock.mock.calls.find(([url]) => url === "/api/v1/rum/report");
    expect(report?.[1]?.body).not.toContain("playbook_run_started");
  });
});
