import type { ReactNode } from "react";
import { PageChromeProvider, usePageChromeHosts } from "@/shared/ui-shared/pageChromeContext";

function Hosts({ children }: { children: ReactNode }) {
  const { setTitleHost, setActionsHost, actionsBlocked } = usePageChromeHosts();
  return <><header data-slot="app-header"><div ref={setTitleHost} /><div ref={setActionsHost} data-slot="page-actions-host" inert={actionsBlocked} aria-hidden={actionsBlocked || undefined} /></header>{children}</>;
}
export function PageChromeFixture({ children }: { children: ReactNode }) {
  return <PageChromeProvider navigationToken="test"><Hosts>{children}</Hosts></PageChromeProvider>;
}
