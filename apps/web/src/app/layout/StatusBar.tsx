import { type MouseEvent, useCallback } from "react";
import { BookText, GitBranch } from "lucide-react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";

import { isPipingDesktopShell, openExternalUrl } from "@/app/desktop/runtime";
import { BUILD_VERSION_LABEL } from "@/shared/lib/build-version";

const StatusBar = () => {
  const { t } = useTranslation();
  const handleExternalLinkClick = useCallback((
    url: string,
    event: MouseEvent<HTMLAnchorElement>,
  ) => {
    if (!isPipingDesktopShell()) return;
    if (
      event.defaultPrevented ||
      event.button !== 0 ||
      event.altKey ||
      event.ctrlKey ||
      event.metaKey ||
      event.shiftKey
    ) {
      return;
    }
    event.preventDefault();
    void openExternalUrl(url).catch((error) => {
      console.error("Failed to open external URL in Piping Desktop.", error);
    });
  }, []);

  return (
    <footer className="min-h-7 flex items-center justify-center px-3 border-t border-border/30 bg-muted/30 text-xs text-muted-foreground shrink-0 select-none gap-3">
      <a
        href="https://piping.space"
        target="_blank"
        rel="noopener noreferrer"
        onClick={(event) => handleExternalLinkClick("https://piping.space", event)}
        className="inline-flex min-h-6 items-center -my-1 px-1 py-1 rounded-sm hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors"
      >
        {t("footer.copyright", { year: new Date().getFullYear() })}
      </a>
      <span className="text-border/50">·</span>
      <span>{BUILD_VERSION_LABEL}</span>
      <span className="text-border/50">·</span>
      <a
        href="https://github.com/mlimarenko/piping"
        target="_blank"
        rel="noopener noreferrer"
        onClick={(event) => handleExternalLinkClick("https://github.com/mlimarenko/piping", event)}
        className="inline-flex min-h-6 items-center gap-1 -my-1 px-1 py-1 rounded-sm hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors"
      >
        <GitBranch className="h-3 w-3" />
        <span>{t("footer.github")}</span>
      </a>
      <span className="text-border/50">·</span>
      <Link
        to="/api-docs"
        className="inline-flex min-h-6 items-center gap-1 -my-1 px-1 py-1 rounded-sm hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors"
      >
        <BookText className="h-3 w-3" />
        <span>{t("footer.apiDocs")}</span>
      </Link>
    </footer>
  );
};

export default StatusBar;
