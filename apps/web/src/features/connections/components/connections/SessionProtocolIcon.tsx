import { LAUNCH_PROTOCOL_ICON, LAUNCH_PROTOCOL_LABEL, type LaunchProtocol } from "./launchPresentation";
import { ExternalLink } from "lucide-react";

export default function SessionProtocolIcon({ protocol, compact = false }: Readonly<{ protocol: LaunchProtocol; compact?: boolean }>) {
  const Icon = LAUNCH_PROTOCOL_ICON[protocol];
  return <span className={compact ? "inline-flex shrink-0 flex-col items-center text-xs font-medium" : "inline-flex shrink-0 items-center gap-1 text-xs font-medium"}><Icon className="size-3.5" aria-hidden="true" />{LAUNCH_PROTOCOL_LABEL[protocol]}{protocol === "app" && <ExternalLink className="size-3" aria-hidden="true" />}</span>;
}
