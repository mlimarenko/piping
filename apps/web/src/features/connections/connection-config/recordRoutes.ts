export const CONNECTION_CONFIG_ROUTE = "/connections/config";

export function connectionConfigRecordHref(id: string): string {
  return `${CONNECTION_CONFIG_ROUTE}/${encodeURIComponent(id)}`;
}
