import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "@/shared/i18n";
import { describe, expect, it, vi } from "vitest";
import { useState } from "react";

import ThresholdEditor from "./ThresholdEditor";
import {
  THRESHOLD_EDITOR_DEFAULTS,
  statusFromThresholdValue,
  thresholdValueFromStatus,
  validateThresholdValue,
  type ThresholdEditorValue,
} from "./thresholdValue";

describe("threshold value helpers", () => {
  it("round-trips a numeric status block through editor state", () => {
    const status = {
      mode: "numeric",
      direction: "higher_is_worse",
      warning: 80,
      critical: 90,
    };
    const value = thresholdValueFromStatus(status);
    expect(value.mode).toBe("numeric");
    expect(value.warning).toBe("80");
    expect(value.critical).toBe("90");
    const rebuilt = statusFromThresholdValue(value);
    expect(rebuilt).toEqual(status);
  });

  it("serializes state mode as token lists", () => {
    const v: ThresholdEditorValue = {
      ...THRESHOLD_EDITOR_DEFAULTS,
      mode: "state",
      normalStates: "up, ok",
      warningStates: "pending",
      criticalStates: "down, 0",
    };
    expect(statusFromThresholdValue(v)).toEqual({
      mode: "state",
      normal: ["up", "ok"],
      warning: ["pending"],
      critical: ["down", "0"],
    });
  });

  it("flags warning >= critical when higher_is_worse", () => {
    const v: ThresholdEditorValue = {
      ...THRESHOLD_EDITOR_DEFAULTS,
      mode: "numeric",
      direction: "higher_is_worse",
      warning: "90",
      critical: "80",
    };
    const t = (_key: string, opts?: Record<string, unknown>) => {
      const defaultValue = opts?.defaultValue;
      return typeof defaultValue === "string" ? defaultValue : "";
    };
    expect(validateThresholdValue(v, t)).toMatch(/smaller than critical/);
  });

  it("accepts auto/none modes without validation", () => {
    const t = (_key: string, opts?: Record<string, unknown>) => {
      const defaultValue = opts?.defaultValue;
      return typeof defaultValue === "string" ? defaultValue : "";
    };
    expect(
      validateThresholdValue({ ...THRESHOLD_EDITOR_DEFAULTS, mode: "auto" }, t),
    ).toBeNull();
    expect(
      validateThresholdValue({ ...THRESHOLD_EDITOR_DEFAULTS, mode: "none" }, t),
    ).toBeNull();
  });
});

function Wrapper() {
  const [value, setValue] = useState<ThresholdEditorValue>({
    ...THRESHOLD_EDITOR_DEFAULTS,
    mode: "numeric",
    warning: "70",
    critical: "90",
  });
  const [sample, setSample] = useState("95");
  return (
    <ThresholdEditor
      value={value}
      onChange={setValue}
      preview={{
        label: "CPU",
        unit: "%",
        sampleText: sample,
        onSampleTextChange: setSample,
      }}
    />
  );
}

describe("ThresholdEditor component", () => {
  it("renders the live preview pill with status color matching thresholds", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { container } = render(
      <QueryClientProvider client={client}>
        <Wrapper />
      </QueryClientProvider>,
    );
    // 95 > 90 critical → status-negative tone
    await waitFor(() => {
      expect(container.querySelector(".text-status-negative")).not.toBeNull();
    });
    expect(screen.getByText(/95/)).toBeInTheDocument();
  });

  it("switches modes via the segmented control", () => {
    const onChange = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <ThresholdEditor
          value={{ ...THRESHOLD_EDITOR_DEFAULTS, mode: "numeric" }}
          onChange={onChange}
        />
      </QueryClientProvider>,
    );
    const tabs = screen.getAllByRole("tab");
    const stateTab = tabs.find((tab) => tab.textContent?.toLowerCase().includes("state"));
    expect(stateTab).toBeDefined();
    fireEvent.click(stateTab!);
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ mode: "state" }));
  });
});
