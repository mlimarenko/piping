export const PLAYBOOK_DETAIL_PATH = "/playbooks/:id";
export const PLAYBOOK_CHILD_SECTIONS = ["versions", "assignments", "schedules", "runs", "targets"] as const;
export type PlaybookChildSection = typeof PLAYBOOK_CHILD_SECTIONS[number];

export function playbookChildSection(value: string | null): PlaybookChildSection {
  return PLAYBOOK_CHILD_SECTIONS.find((section) => section === value) ?? "versions";
}

export function playbookDetailHref(id: string, section: PlaybookChildSection = "versions", search = ""): string {
  const params = new URLSearchParams(search);
  params.set("section", section);
  return `/playbooks/${encodeURIComponent(id)}?${params}`;
}

/** Return destinations stay inside the playbook UI, never a supplied origin. */
export function playbookReturnHref(value: string | null): string {
  if (!value || !/^\/playbooks(?:\?|\/|$)/.test(value)) return "/playbooks";
  const url = new URL(value, "https://playbooks.invalid");
  return url.origin === "https://playbooks.invalid" && /^\/playbooks(?:\/|$)/.test(url.pathname)
    ? url.pathname + url.search + url.hash : "/playbooks";
}
