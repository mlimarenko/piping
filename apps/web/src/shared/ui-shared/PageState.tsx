import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { ArrowLeft } from "lucide-react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import PageChrome from "./PageChrome";

interface PageStateProps {
  title: string;
  icon?: LucideIcon;
  backHref: string;
  backLabel?: string;
  children: ReactNode;
}

/** A routed loading or unavailable view retains its page heading and navigation. */
export default function PageState({ title, icon, backHref, backLabel, children }: PageStateProps) {
  const { t } = useTranslation();
  return <div className="flex min-w-0 flex-col gap-4">
    <PageChrome title={title} icon={icon} actions={<Button asChild variant="ghost">
      <Link to={backHref}><ArrowLeft className="size-4" />{backLabel ?? t("back")}</Link>
    </Button>} />
    {children}
  </div>;
}
