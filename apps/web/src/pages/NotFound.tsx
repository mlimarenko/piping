import { Link } from "react-router";
import { useTranslation } from "react-i18next";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import { Button } from "@/shared/ui-primitives/button";
import { AlertCircle } from "lucide-react";

const NotFound = () => {
  const { t } = useTranslation();
  return (
    // The e2e route smoke asserts on this marker: NotFound renders its own <h1>,
    // so a "page rendered a heading" check passes on a dead route. A stable,
    // language-neutral marker is the only way that suite can tell a real page
    // from the catch-all.
    <div
      data-testid="page-not-found"
      className="flex min-h-full items-center justify-center bg-background p-4"
    >
      <SurfaceCard className="w-full max-w-sm p-6 text-center">
        <AlertCircle className="h-8 w-8 text-muted-foreground mx-auto mb-3" />
        <h1 className="mb-2 text-xl font-semibold text-foreground">{t("notFound.title")}</h1>
        <p className="mb-4 text-sm text-muted-foreground">{t("notFound.description")}</p>
        <Button asChild size="sm">
          <Link to="/connections">{t("notFound.backToWorkspace")}</Link>
        </Button>
      </SurfaceCard>
    </div>
  );
};

export default NotFound;
