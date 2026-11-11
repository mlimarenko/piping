/** Return destinations stay on current automation routes, never a supplied origin. */
export function playbookReturnHref(value: string | null): string {
  if (!value || !/^\/playbooks\//.test(value)) return "/playbooks/schedules";
  const url = new URL(value, "https://playbooks.invalid");
  return url.origin === "https://playbooks.invalid" && /^\/playbooks\/(?:projects|jobs|runs|schedules|settings)(?:\/|$)/.test(url.pathname)
    ? url.pathname + url.search + url.hash : "/playbooks/schedules";
}
