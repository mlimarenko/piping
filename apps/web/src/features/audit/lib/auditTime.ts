const DASH = "—";

const DATE_TIME_OPTIONS: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  timeZoneName: "short",
};

export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

export function formatAuditTimestamp(
  value: string | null | undefined,
  locale: string,
  timeZone = browserTimeZone(),
): string {
  if (!value) return DASH;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return DASH;

  try {
    return new Intl.DateTimeFormat(locale, {
      ...DATE_TIME_OPTIONS,
      timeZone,
    }).format(date);
  } catch {
    return new Intl.DateTimeFormat(locale, DATE_TIME_OPTIONS).format(date);
  }
}

export function formatAuditTimestampUtc(value: string | null | undefined): string {
  if (!value) return DASH;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return DASH;
  return date.toISOString();
}

