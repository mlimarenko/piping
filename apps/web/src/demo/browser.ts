/**
 * Browser-side MSW worker for demo mode. Boots the service worker that
 * intercepts every `/api/v1/*` request and answers from `handlers.ts`.
 */
import { setupWorker } from "msw/browser";
import { handlers } from "./handlers";

export const worker = setupWorker(...handlers);
