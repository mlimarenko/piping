import type { PlaybookLaunchIntent } from "@/features/playbooks/PlaybookLaunchDialog";

function objectRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function resolvedConnection(intent: PlaybookLaunchIntent, launchOptions: Record<string, unknown>): string {
  const candidate = launchOptions.connection;
  if (typeof candidate === "string" && candidate.trim()) {
    return candidate;
  }
  return intent.playbook?.playbook.connection_default ?? "ssh";
}

export function buildStaticPlaybookRunMetadata(
  intent: PlaybookLaunchIntent,
  source: string,
  targetCount: number,
): Record<string, unknown> {
  const metadata = objectRecord(intent.metadata);
  const launchOptions = objectRecord(metadata.launch_options);
  const hasTargetLimit = Object.prototype.hasOwnProperty.call(launchOptions, "target_limit");

  return {
    ...metadata,
    source,
    target_count: targetCount,
    launch_options: {
      ...launchOptions,
      connection: resolvedConnection(intent, launchOptions),
      credential_choice: launchOptions.credential_choice ?? intent.credentialChoice ?? "auto",
      ...(hasTargetLimit ? {} : { target_limit: intent.targetLimit ?? targetCount }),
    },
  };
}
