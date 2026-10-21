import { describe, expect, it } from "vitest";

import {
  clampConnectionsPanelSize,
  findPreferredPanelSession,
  normalizeConnectionsPanelState,
} from "./panelState";
import { defaultSessionPaneLayout, splitSessionPane } from "./sessionPaneLayout";

describe("connections panel persisted state", () => {
  it("persists only allowlisted layout metadata under the existing panel preference", () => {
    const paneLayout = splitSessionPane(defaultSessionPaneLayout("session-1"), "vertical");
    expect(normalizeConnectionsPanelState({ paneLayout }, false).paneLayout).toEqual(paneLayout);
    expect(normalizeConnectionsPanelState({ paneLayout: { ...paneLayout, token: "rejected-fixture" } }, false).paneLayout).toBeNull();
    expect(normalizeConnectionsPanelState({ paneLayout }, true).paneLayout).toEqual(paneLayout);
  });
  it("normalizes saved dock, collapsed state, active tab and per-dock sizes", () => {
    const state = normalizeConnectionsPanelState(
      {
        showPanel: true,
        expanded: false,
        dockPosition: "right",
        sizes: { bottom: 9999, right: 10 },
        activeSession: {
          sessionId: "session-1",
          hostId: "host-1",
          protocol: "ssh",
        },
      },
      false,
    );

    expect(state.showPanel).toBe(true);
    expect(state.expanded).toBe(false);
    expect(state.dockPosition).toBe("right");
    expect(state.sizes.right).toBe(320);
    expect(state.sizes.bottom).toBeLessThanOrEqual(Math.floor(window.innerHeight * 0.85));
    expect(state.activeSession).toEqual({
      sessionId: "session-1",
      hostId: "host-1",
      protocol: "ssh",
    });
  });

  it("falls back from stale session id to the latest matching host/protocol tab", () => {
    const sessions = [
      { id: "old", hostId: "host-1", protocol: "ssh" as const },
      { id: "rdp", hostId: "host-1", protocol: "rdp" as const },
      { id: "new", hostId: "host-1", protocol: "ssh" as const },
    ];

    expect(
      findPreferredPanelSession(sessions, {
        sessionId: "missing",
        hostId: "host-1",
        protocol: "ssh",
      })?.id,
    ).toBe("new");
  });

  it("caps right-docked panel size to the measured workspace space", () => {
    expect(clampConnectionsPanelSize("right", 900, 440)).toBe(440);
    expect(clampConnectionsPanelSize("right", 900, 200)).toBe(320);
  });
});
