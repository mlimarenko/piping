import { ApiError } from "@/shared/api/client";
import { sshCloseMessage } from "@/features/connections/sshCloseMessage";

export type ConnectionFailureKind =
  | "authentication"
  | "configuration"
  | "unreachable"
  | "timeout"
  | "unsupported"
  | "disconnected"
  | "busy"
  | "network"
  | "unknown";

export interface ConnectionFailure {
  kind: ConnectionFailureKind;
}

const REASON_KIND: Readonly<Record<string, ConnectionFailureKind>> = {
  connection_credentials_required: "configuration",
  connection_authentication_failed: "authentication",
  connection_timeout: "timeout",
  connection_unreachable: "unreachable",
  connection_busy: "busy",
  connection_unsupported: "unsupported",
  connection_failed: "unknown",
};

function normalizeReasonCode(value: string | undefined): string | undefined {
  return value?.trim().toLowerCase().replaceAll("-", "_");
}

export function connectionFailureFromApiError(error: unknown): ConnectionFailure {
  if (!(error instanceof ApiError)) return { kind: "unknown" };

  const reasonCode = normalizeReasonCode(error.reasonCode);
  if (reasonCode && REASON_KIND[reasonCode]) {
    return { kind: REASON_KIND[reasonCode] };
  }
  return { kind: "unknown" };
}

/**
 * Compatibility adapter for pre-reason-code WebSocket close frames. The exact
 * literals mirror the backend's legacy, protocol-defined close vocabulary.
 * Unknown prose is deliberately ignored so backend details never reach the UI.
 */
export function connectionFailureFromWsClose(
  code: number | undefined,
  reason: string | undefined,
): ConnectionFailure {
  if (code === 1013 && reason?.trim() === "Target connection timed out") {
    return { kind: "timeout" };
  }

  const { titleKey } = sshCloseMessage(code, reason);
  const kindByTitleKey: Readonly<Record<string, ConnectionFailureKind>> = {
    "connections.authFailed": "authentication",
    "connections.hostUnreachable": "unreachable",
    "connections.upstreamLost": "disconnected",
    "connections.networkError": "network",
    "connections.connectionFailed": "unknown",
  };
  return { kind: kindByTitleKey[titleKey] ?? "unknown" };
}
