import { lazy, Suspense, useState } from "react";
import { useLocation } from "react-router";
import { RequireCapability } from "@/shared/auth/permissions";

const ConnectionsWorkspace = lazy(() => import("./ConnectionsWorkspace"));

/** A running workspace belongs to the authenticated shell, not to an editor route. */
export default function ConnectionsWorkspaceOutlet() {
  const visible = useLocation().pathname === "/connections";
  const [visited, setVisited] = useState(visible);
  if (visible && !visited) setVisited(true);
  if (!visited) return null;
  return <div hidden={!visible} inert={!visible} className="h-full min-h-0 min-w-0" aria-hidden={!visible}>
    <RequireCapability visible={visible} check={permissions => permissions.can("READ", "CONNECTION_TARGET")}>
      <Suspense fallback={null}><ConnectionsWorkspace visible={visible} /></Suspense>
    </RequireCapability>
  </div>;
}
