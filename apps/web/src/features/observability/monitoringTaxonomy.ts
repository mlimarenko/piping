export const MONITORING_SOURCE_TYPES = ["prometheus", "victoriametrics", "zabbix"] as const;
// Alert-side source families the poller pulls active alerts from. Mirrors the
// backend `MONITORING_ALERT_SOURCE_TYPES`; `alertmanager` is the default.
export const MONITORING_ALERT_SOURCE_TYPES = ["alertmanager", "vmalert", "zabbix"] as const;
export const PROMQL_QUERY_TYPES = ["promql"] as const;
export const ZABBIX_QUERY_TYPES = [
  "zabbix_item_key",
  "zabbix_item_name",
  "zabbix_itemid",
] as const;

export type MonitoringSourceType = (typeof MONITORING_SOURCE_TYPES)[number];

export function isMonitoringSourceType(value: string): value is MonitoringSourceType {
  return MONITORING_SOURCE_TYPES.includes(value as MonitoringSourceType);
}

export type MonitoringAlertSourceType = (typeof MONITORING_ALERT_SOURCE_TYPES)[number];

export function isMonitoringAlertSourceType(value: string): value is MonitoringAlertSourceType {
  return MONITORING_ALERT_SOURCE_TYPES.includes(value as MonitoringAlertSourceType);
}
export function isZabbixSourceType(sourceType: string): boolean {
  return sourceType === "zabbix";
}

export function queryTypesForSourceType(sourceType: string): readonly string[] {
  return isZabbixSourceType(sourceType) ? ZABBIX_QUERY_TYPES : PROMQL_QUERY_TYPES;
}

export function defaultQueryTypeForSourceType(sourceType: string): string {
  return isZabbixSourceType(sourceType) ? "zabbix_item_key" : "promql";
}

export function isMetricQueryTypeCompatible(sourceType: string, queryType: string): boolean {
  return queryTypesForSourceType(sourceType).includes(queryType);
}
