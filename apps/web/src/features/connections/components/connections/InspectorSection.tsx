import { ChevronDown, ChevronRight } from "lucide-react";

/**
 * Collapsible section shell shared across the connections inspector: a toggle
 * header (icon + title + chevron) with an optional sibling `action` (e.g. a "+"
 * button) and a body shown only when expanded. The action is a SIBLING of the
 * toggle button — never nested inside it — so the DOM stays valid and the
 * toggle's accessible name is unchanged.
 *
 * Single source of truth for the section header/toggle pattern: both
 * {@link TreeNodeInspector}'s own sections and {@link RelatedEntitySection} use
 * this shell, so there is exactly one collapsible-row markup in the inspector.
 */
const InspectorSection = ({ title, icon: Icon, sectionKey, expanded, onToggle, action, children }: {
  title: React.ReactNode;
  icon: React.ElementType;
  sectionKey: string;
  expanded: string[];
  onToggle: (k: string) => void;
  /** Optional header action rendered as a SIBLING of the toggle button. */
  action?: React.ReactNode;
  children: React.ReactNode;
}) => {
  const isExpanded = expanded.includes(sectionKey);
  return (
    <div className="border-b border-border/20">
      <div className="flex items-center pr-1 hover:bg-accent/20 transition-colors">
        <button type="button" onClick={() => onToggle(sectionKey)} aria-expanded={isExpanded}
          className="flex min-h-9 flex-1 items-center gap-2 px-3 py-2 text-sm font-medium text-foreground">
          <Icon className="size-4 text-muted-foreground" />
          <span className="flex-1 text-left">{title}</span>
          {isExpanded ? <ChevronDown className="h-3 w-3 text-muted-foreground" /> : <ChevronRight className="h-3 w-3 text-muted-foreground" />}
        </button>
        {action}
      </div>
      {isExpanded && <div className="px-3 pb-2 space-y-1">{children}</div>}
    </div>
  );
};

export default InspectorSection;
