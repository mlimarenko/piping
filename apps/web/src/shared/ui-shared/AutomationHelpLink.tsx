import { BookOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { automationDocUrl, type AutomationDocSlug } from "@/shared/lib/automationDocs";

/** The single help-link affordance for an automation screen or field. */
export function AutomationHelpLink({ article, hash }: Readonly<{
  article: AutomationDocSlug;
  hash?: string;
}>) {
  const { t } = useTranslation();
  return (
    <Button asChild variant="ghost" size="sm" className="gap-1.5">
      <a href={automationDocUrl(article, hash)} target="_blank" rel="noopener noreferrer">
        <BookOpen className="size-3.5" aria-hidden="true" />
        {t("playbooks.page.help.docs")}
        <span className="sr-only">{t("playbooks.help.newTab")}</span>
      </a>
    </Button>
  );
}
