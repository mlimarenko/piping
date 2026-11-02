import { asParams, isConnConfProtocol, type ConnConfParams, type ConnConfProtocol, type ConnConfRecord } from "./api";

export const DEFAULT_ENDPOINT_HOST = "{{primary_ip4}}";

export interface RecordDraft {
  name: string;
  description: string;
  protocol: ConnConfProtocol;
  baseRecordId: string;
  enabled: boolean;
  params: ConnConfParams;
}

export function emptyDraft(protocol: ConnConfProtocol = "ssh"): RecordDraft {
  return {
    name: "",
    description: "",
    protocol,
    baseRecordId: "",
    enabled: true,
    params: protocol === "app" ? {} : { host: DEFAULT_ENDPOINT_HOST },
  };
}

export function draftFromRecord(record: ConnConfRecord): RecordDraft {
  const protocol: ConnConfProtocol = isConnConfProtocol(record.protocol)
    ? record.protocol
    : "ssh";
  return {
    name: record.name,
    description: record.description,
    protocol,
    baseRecordId: record.base_record_id ?? "",
    enabled: record.enabled,
    params: asParams(record.params),
  };
}

/** The value if it is a non-empty object, else undefined — so an empty carried
 * subtree is dropped from the rebuilt params instead of persisting as `{}`. */
function nonEmpty<T extends object>(value: T | undefined): T | undefined {
  return value && Object.keys(value).length ? value : undefined;
}

export function paramsForProtocolChange(
  protocol: ConnConfProtocol,
  current: ConnConfParams,
): ConnConfParams {
  const ui = nonEmpty(current.ui);
  const uiPart = ui ? { ui } : {};

  if (protocol === "app") {
    return { ...(current.url ? { url: current.url } : {}), ...uiPart };
  }

  if (protocol === "cctv") {
    // Video accepts no timeout_secs (rejected by the backend), so unlike the
    // generic session branch we drop it; credentials (username/password) carry
    // over since they are the device-management auth.
    const credentials = nonEmpty(current.credentials);
    return {
      host: current.host || DEFAULT_ENDPOINT_HOST,
      ...(credentials ? { credentials } : {}),
      ...uiPart,
    };
  }

  return {
    host: current.host || DEFAULT_ENDPOINT_HOST,
    ...(current.timeout_secs != null ? { timeout_secs: current.timeout_secs } : {}),
    ...uiPart,
  };
}
