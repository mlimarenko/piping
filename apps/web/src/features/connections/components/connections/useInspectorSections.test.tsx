import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { writeBrowserLocalSetting } from "@/shared/settings/local-cache";
import { useInspectorSections } from "./useInspectorSections";

describe("browser-local inspector sections", () => {
  beforeEach(() => window.localStorage.clear());

  it("starts collapsed and restores expansion and subsequent collapse after remount", () => {
    const first = renderHook(() => useInspectorSections());
    expect(first.result.current.expandedSections).toEqual([]);
    act(() => first.result.current.toggleSection("details"));
    act(() => first.result.current.toggleSection("contacts"));
    first.unmount();
    const second = renderHook(() => useInspectorSections());
    expect(second.result.current.expandedSections).toEqual(["details", "contacts"]);
    act(() => second.result.current.toggleSection("details"));
    second.unmount();
    expect(renderHook(() => useInspectorSections()).result.current.expandedSections).toEqual(["contacts"]);
  });

  it("normalizes invalid stored data without opening anything by default", () => {
    writeBrowserLocalSetting("connections.inspector.expandedSections", { details: true });
    expect(renderHook(() => useInspectorSections()).result.current.expandedSections).toEqual([]);
  });

  it("still toggles when storage writes are unavailable", () => {
    const write = vi.spyOn(window.localStorage, "setItem").mockImplementation(() => { throw new Error("quota"); });
    const { result } = renderHook(() => useInspectorSections());
    act(() => result.current.toggleSection("overview"));
    expect(result.current.expandedSections).toEqual(["overview"]);
    write.mockRestore();
  });
});
