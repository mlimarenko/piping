import * as Sentry from "@sentry/react";

import type { CaptureContext, SentryBridge } from "./observability";

/**
 * The ONLY module allowed to import `@sentry/react`.
 *
 * It is reached exclusively through the dynamic `import()` in
 * `./observability.ts`, which keeps the whole SDK — including
 * `replayIntegration`, the single largest piece — out of the initial payload.
 * Replay stays statically imported *here* on purpose: `lazyLoadIntegration`
 * would fetch it from Sentry's CDN, which a self-hosted deployment behind a
 * strict CSP cannot reach.
 */

export interface SentryOptions {
  dsn: string;
  environment: string;
  release: string | undefined;
  tracesSampleRate: number;
}

export function startSentry(options: SentryOptions): SentryBridge {
  Sentry.init({
    dsn: options.dsn,
    environment: options.environment,
    release: options.release,
    integrations: [Sentry.browserTracingIntegration(), Sentry.replayIntegration()],
    tracesSampleRate: options.tracesSampleRate,
    replaysSessionSampleRate: 0,
    replaysOnErrorSampleRate: 1.0,
    sendDefaultPii: false,
  });

  return {
    capture(error: unknown, context: CaptureContext): void {
      Sentry.withScope((scope) => {
        for (const [key, value] of Object.entries(context.tags ?? {})) {
          scope.setTag(key, value);
        }
        for (const [key, value] of Object.entries(context.extras ?? {})) {
          scope.setExtra(key, value);
        }
        Sentry.captureException(error);
      });
    },
    tagRequestId(requestId: string): void {
      Sentry.getCurrentScope().setTag("traceId", requestId);
    },
  };
}
