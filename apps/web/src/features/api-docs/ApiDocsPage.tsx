import SwaggerUI from "swagger-ui-react";
import type { ComponentProps } from "react";
import { useTranslation } from "react-i18next";
import PageChrome from "@/shared/ui-shared/PageChrome";
import SwaggerInfo, { type SwaggerInfoPlugin } from "./SwaggerInfo";
import "swagger-ui-react/swagger-ui.css";
import "./api-docs.css";

// The local SDK declaration omits its documented plugins prop. Keep that
// strictly typed boundary here without replacing the public React component.
const swaggerUiProps = {
  url: "/api/v1/openapi/piping.openapi.yaml",
  plugins: [{ components: { OAS31Info: SwaggerInfo } }],
} satisfies ComponentProps<typeof SwaggerUI> & { plugins: SwaggerInfoPlugin[] };

const ApiDocsPage = () => {
  const { t } = useTranslation();
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome title={t("routeTitles./api-docs")} />
      <div className="api-docs-scope min-w-0 overflow-x-auto rounded-lg border border-border bg-card p-4 md:p-6">
        <SwaggerUI {...swaggerUiProps} />
      </div>
    </div>
  );
};

export default ApiDocsPage;
