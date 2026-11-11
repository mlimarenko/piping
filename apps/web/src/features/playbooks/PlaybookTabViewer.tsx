import type { PlaybookTab } from "@/features/playbooks/playbookTabs";
import { RunDetailPanel } from "@/features/playbooks/RunDetailPanel";

export interface PlaybookTabViewerProps {
  tab: PlaybookTab;
}

/**
 * Thin wrapper around `RunDetailPanel` for the Connections tab viewer switch.
 * No `onClose` — closing a playbook tab is handled by the tab
 * strip's own close button, not the panel header.
 */
function PlaybookTabViewer({ tab }: PlaybookTabViewerProps) {
  return (
    <RunDetailPanel
      compact
      target={tab.target}
      playbookName={tab.playbookName}
      className="h-full"
    />
  );
}

export default PlaybookTabViewer;
