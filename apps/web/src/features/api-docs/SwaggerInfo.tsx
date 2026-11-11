import { createElement, type ComponentType, type ReactNode } from "react";

interface SwaggerComponents {
  Markdown: { source?: string };
  Link: { href: string; target?: string; className?: string; children: ReactNode };
  VersionStamp: { version: string };
  OpenAPIVersion: { oasVersion: string };
  InfoUrl: { url: string; getComponent: GetComponent };
  InfoBasePath: { host?: string; basePath?: string };
  License: object;
  Contact: object;
  JsonSchemaDialect: object;
}

type GetComponent = <Name extends keyof SwaggerComponents>(
  name: Name,
  container?: boolean,
) => ComponentType<SwaggerComponents[Name]>;

interface InfoProps {
  getComponent: GetComponent;
  specSelectors: {
    version(): unknown;
    url(): unknown;
    basePath(): unknown;
    host(): unknown;
    selectInfoSummaryField(): unknown;
    selectInfoDescriptionField(): unknown;
    selectInfoTitleField(): unknown;
    selectInfoTermsOfServiceUrl(): unknown;
    selectExternalDocsUrl(): unknown;
    selectExternalDocsDescriptionField(): unknown;
    contact(): { size: number };
    license(): { size: number };
  };
}

function text(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

// Swagger selectors resolve relative URLs. Only web links belong in these fields.
function webUrl(value: unknown): string | undefined {
  if (typeof value !== "string" || !value) return undefined;
  try {
    const url = new URL(value, window.location.href);
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : undefined;
  } catch {
    return undefined;
  }
}

/** Swagger UI 5.32's OAS31Info contract, with a section heading under PageChrome. */
export default function SwaggerInfo({ getComponent, specSelectors }: InfoProps) {
  const version = text(specSelectors.version());
  const url = webUrl(specSelectors.url());
  const host = text(specSelectors.host());
  const basePath = text(specSelectors.basePath());
  const summary = text(specSelectors.selectInfoSummaryField());
  const description = text(specSelectors.selectInfoDescriptionField());
  const title = text(specSelectors.selectInfoTitleField());
  const termsOfServiceUrl = webUrl(specSelectors.selectInfoTermsOfServiceUrl());
  const externalDocsUrl = webUrl(specSelectors.selectExternalDocsUrl());
  const externalDocsDescription = text(specSelectors.selectExternalDocsDescriptionField());
  // These are registered SDK components, not component definitions created by this render.
  // Instantiate the registry's component types explicitly through React's element API.
  const Markdown = getComponent("Markdown", true);
  const Link = getComponent("Link");
  const VersionStamp = getComponent("VersionStamp");
  const OpenAPIVersion = getComponent("OpenAPIVersion");
  const InfoUrl = getComponent("InfoUrl");
  const InfoBasePath = getComponent("InfoBasePath");
  const License = getComponent("License", true);
  const Contact = getComponent("Contact", true);
  const JsonSchemaDialect = getComponent("JsonSchemaDialect", true);

  return (
    <div className="info">
      <hgroup className="main">
        <h2 className="title">
          {title}
          <span>
            {version && createElement(VersionStamp, { version })}
            {createElement(OpenAPIVersion, { oasVersion: "3.1" })}
          </span>
        </h2>
        {(host || basePath) && createElement(InfoBasePath, { host, basePath })}
        {url && createElement(InfoUrl, { getComponent, url })}
      </hgroup>
      {summary && <p className="info__summary">{summary}</p>}
      <div className="info__description description">{createElement(Markdown, { source: description })}</div>
      {termsOfServiceUrl && (
        <div className="info__tos">{createElement(Link, { target: "_blank", href: termsOfServiceUrl, children: "Terms of service" })}</div>
      )}
      {specSelectors.contact().size > 0 && createElement(Contact)}
      {specSelectors.license().size > 0 && createElement(License)}
      {externalDocsUrl && (
        createElement(Link, { className: "info__extdocs", target: "_blank", href: externalDocsUrl, children: externalDocsDescription || externalDocsUrl })
      )}
      {createElement(JsonSchemaDialect)}
    </div>
  );
}

export interface SwaggerInfoPlugin {
  components: { OAS31Info: ComponentType<InfoProps> };
}
