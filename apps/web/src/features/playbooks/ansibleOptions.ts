export interface AnsibleOptions {
  tags: string;
  skipTags: string;
  check: boolean;
  diff: boolean;
  verbosity: number;
  become: boolean;
  becomeUser: string;
  becomeMethod: string;
}

export const emptyAnsibleOptions = (): AnsibleOptions => ({ tags: "", skipTags: "", check: false, diff: false, verbosity: 0, become: false, becomeUser: "", becomeMethod: "" });

/** Projection of the wire argv; the shared backend/runner parser enforces it. */
export function decodeAnsibleOptions(text: string): AnsibleOptions | null {
  let args: unknown;
  try { args = JSON.parse(text); } catch { return null; }
  if (!Array.isArray(args) || !args.every((value): value is string => typeof value === "string")) return null;
  const result = emptyAnsibleOptions();
  const seen = new Set<keyof AnsibleOptions>();
  for (let i = 0; i < args.length; i++) {
    const argument = args[i];
    if (argument === undefined) return null;
    let name = argument;
    let attached: string | undefined;
    if (argument.startsWith("--") && argument.includes("=")) {
      const split = argument.indexOf("=");
      name = argument.slice(0, split); attached = argument.slice(split + 1);
    } else if (argument.startsWith("-t") && argument.length > 2) {
      name = "-t"; attached = argument.slice(2);
    }
    let key: keyof AnsibleOptions;
    if (["--check", "-C", "--diff", "-D", "--become", "-b"].includes(name) && attached === undefined) {
      key = name === "--check" || name === "-C" ? "check" : name === "--diff" || name === "-D" ? "diff" : "become";
      result[key] = true;
    } else if (/^-v{1,4}$/.test(argument) || argument === "--verbose") {
      key = "verbosity"; result.verbosity = argument === "--verbose" ? 1 : argument.length - 1;
    } else if (["--tags", "-t", "--skip-tags", "--become-user", "--become-method"].includes(name)) {
      const value = attached ?? args[++i];
      if (!value || /\p{Cc}/u.test(value)) return null;
      key = name === "--tags" || name === "-t" ? "tags" : name === "--skip-tags" ? "skipTags" : name === "--become-user" ? "becomeUser" : "becomeMethod";
      result[key] = value;
    } else return null;
    if (seen.has(key)) return null;
    seen.add(key);
  }
  return !result.become && (result.becomeUser || result.becomeMethod) ? null : result;
}

export function encodeAnsibleOptions(options: AnsibleOptions): string {
  const args: string[] = [];
  for (const [name, value] of [["--tags", options.tags], ["--skip-tags", options.skipTags], ["--become-user", options.becomeUser], ["--become-method", options.becomeMethod]]) {
    if (value) args.push(`${name}=${value}`);
  }
  if (options.check) args.push("--check");
  if (options.diff) args.push("--diff");
  if (options.become) args.push("--become");
  if (options.verbosity) args.push(`-${"v".repeat(options.verbosity)}`);
  return JSON.stringify(args, null, 2);
}
