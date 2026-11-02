export type LiveSessionProtocol = "ssh" | "rdp" | "vnc" | "cctv";

export const loadLiveTerminal = () =>
  import("@/features/connections/components/connections/LiveTerminal");

export const loadLiveRDP = () =>
  import("@/features/connections/components/connections/LiveRDP");

export const loadLiveVNC = () =>
  import("@/features/connections/components/connections/LiveVNC");

export const loadCctvConsole = () =>
  import("@/features/device-dashboard/CctvConsole");

export function preloadLiveSessionProtocol(protocol: LiveSessionProtocol): Promise<unknown> {
  switch (protocol) {
    case "ssh":
      return loadLiveTerminal();
    case "rdp":
      return loadLiveRDP();
    case "vnc":
      return loadLiveVNC();
    case "cctv":
      return loadCctvConsole();
  }
}
