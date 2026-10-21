import {
  Bot,
  Cog,
  KeyRound,
  PlayCircle,
  ServerCog,
  UserRound,
  Webhook,
  type LucideIcon,
} from "lucide-react";

/**
 * Ordered actor-channel (`actor_kind`) options for the audit channel filter and
 * the actor-channel badge. Values match the backend `audit_event.actor_kind`
 * CHECK domain; human labels come from `auditLog.actorKinds.*` i18n so the
 * filter, the badge, and the "via" line all read from one source.
 */
export const AUDIT_CHANNELS = [
  { value: "user", icon: UserRound },
  { value: "mcp_agent", icon: Bot },
  { value: "api_token", icon: KeyRound },
  { value: "playbook_job_token", icon: PlayCircle },
  { value: "service_principal", icon: ServerCog },
  { value: "webhook", icon: Webhook },
  { value: "system", icon: Cog },
] as const;

const CHANNEL_ICON = new Map<string, LucideIcon>(
  AUDIT_CHANNELS.map((channel) => [channel.value, channel.icon]),
);

/**
 * Icon for an `actor_kind`, defaulting to a neutral user glyph for unknown or
 * missing kinds (older rows, future kinds not yet in {@link AUDIT_CHANNELS}).
 */
export function auditChannelIcon(kind: string | undefined): LucideIcon {
  return (kind ? CHANNEL_ICON.get(kind) : undefined) ?? UserRound;
}
