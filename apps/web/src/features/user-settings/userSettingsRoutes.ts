export const USER_SETTINGS_ROUTE = "/user/settings";

export const USER_CREDENTIAL_NEW_ROUTE = `${USER_SETTINGS_ROUTE}/credentials/new`;

export const USER_TOKEN_NEW_ROUTE = `${USER_SETTINGS_ROUTE}/tokens/new`;

export function userCredentialEditHref(id: string): string {
  return `${USER_SETTINGS_ROUTE}/credentials/${encodeURIComponent(id)}/edit`;
}

type AccountContext = { search: string; hash: string };

export function userSettingsEditorHref(path: string, context: AccountContext): string {
  const params = new URLSearchParams(context.search);
  params.delete("section");
  params.delete("create");
  const search = params.toString();
  return `${path}${search ? `?${search}` : ""}${context.hash}`;
}

export function userSettingsSectionHref(section: string, context?: AccountContext): string {
  const params = new URLSearchParams(context?.search);
  params.delete("section");
  params.delete("create");
  params.set("section", section);
  return `${USER_SETTINGS_ROUTE}?${params.toString()}${context?.hash ?? ""}`;
}
