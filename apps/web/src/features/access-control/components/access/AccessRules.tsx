import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, Search } from "lucide-react";
import AccessCheck from "./AccessCheck";
import AccessPolicies from "./AccessPolicies";

/**
 * The "Access Rules" tab: the full policy list (AccessPolicies) with an
 * authoritative, collapsible "Test access" panel folded in. The policy list is
 * the completeness authority for every policy regardless of subject kind or
 * effect.
 */
const AccessRules = () => {
  const { t } = useTranslation();
  const [testOpen, setTestOpen] = useState(false);

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <div className="shrink-0">
        <button
          type="button"
          aria-expanded={testOpen}
          aria-controls="access-test-panel"
          onClick={() => setTestOpen((o) => !o)}
          className="flex w-full items-center gap-2 rounded-lg border border-input/30 bg-card px-4 py-2.5 text-left transition-colors hover:bg-accent"
        >
          <Search className="size-4 shrink-0 text-primary" />
          <span className="text-sm font-medium text-foreground">{t("accessControl.rules.testAccess")}</span>
          <span className="hidden truncate text-xs text-muted-foreground sm:inline">— {t("accessControl.rules.testAccessHint")}</span>
          {testOpen
            ? <ChevronDown className="ml-auto h-4 w-4 shrink-0 text-muted-foreground" />
            : <ChevronRight className="ml-auto h-4 w-4 shrink-0 text-muted-foreground" />}
        </button>
        {testOpen && (
          <div id="access-test-panel" role="region" aria-label={t("accessControl.rules.testAccess")} className="mt-3">
            <AccessCheck />
          </div>
        )}
      </div>
      <AccessPolicies />
    </div>
  );
};

export default AccessRules;
