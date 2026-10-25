import { useRef, useState, type ReactNode } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Trans, useTranslation } from "react-i18next";
import { ArrowLeft, Pencil, Radio, Trash2 } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import { Button } from "@/shared/ui-primitives/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import PageChrome from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import PageState from "@/shared/ui-shared/PageState";
import StatusChip from "@/shared/ui-shared/StatusChip";
import { notify } from "@/shared/lib/notifications";
import { deleteProxyNode, listProxyNodes } from "@/features/connections/proxies-api";
import { PROXY_NODES_ROUTE, proxyNodeEditHref } from "./proxyRecordRoutes";

export default function ProxyNodeDetail() {
  const { id } = useParams();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [deleteOpen, setDeleteOpen] = useState(false);
  const actionTrigger = useRef<HTMLButtonElement>(null);
  const [error, setError] = useState("");
  const query = useQuery({
    queryKey: ["proxy-nodes", "options"],
    queryFn: () => listProxyNodes({ limit: 1000, offset: 0 }),
  });
  const mutation = useApiMutation({
    mutationFn: (nodeId: string) => deleteProxyNode(nodeId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["proxy-nodes"] });
      notify("success", t("proxyNodes.nodeDeleted"));
      void navigate(PROXY_NODES_ROUTE);
    },
    onError: (err: Error) => setError(err.message || t("unknownError")),
  });

  const renderState = (content: ReactNode) => <><PageChrome title={t("proxyNodes.title")} icon={Radio} actions={<Button asChild variant="ghost"><Link to={PROXY_NODES_ROUTE}><ArrowLeft className="size-4" />{t("proxyNodes.backToList")}</Link></Button>} />{content}</>;
  if (query.isPending) return renderState(<PageState kind="loading" title={t("loading")} />);
  if (query.isError) return renderState(<PageState kind="error" title={t("proxyNodes.loadFailed")} action={<Button variant="outline" onClick={() => { void query.refetch(); }}>{t("retry")}</Button>} />);
  const node = query.data.items.find(candidate => candidate.id === id);
  if (!node) return renderState(<PageState kind="empty" title={t("proxyNodes.notFound")} />);
  const rotatedAt = node.secret_rotated_at
    ? new Date(node.secret_rotated_at).toLocaleString()
    : t("proxyNodes.neverRotated");
  return <div className="flex min-w-0 flex-col gap-4">
    <PageChrome title={node.name} description={node.base_url} icon={Radio} actions={<>
      <Button asChild variant="ghost"><Link to={PROXY_NODES_ROUTE}><ArrowLeft className="size-4" />{t("proxyNodes.backToList")}</Link></Button>
      {(can("UPDATE", "PROXY_NODE") || can("DELETE", "PROXY_NODE")) && <RowActions label={`${t("actions")}: ${node.name}`} ref={actionTrigger}>
        {can("UPDATE", "PROXY_NODE") && <DropdownMenuItem asChild><Link to={proxyNodeEditHref(node.id)}><Pencil aria-hidden="true" />{t("edit")}</Link></DropdownMenuItem>}
        {can("DELETE", "PROXY_NODE") && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteOpen(true)} disabled={mutation.isPending}><Trash2 aria-hidden="true" />{t("delete")}</DropdownMenuItem>}
      </RowActions>}
    </>} />
    {error && <p role="alert" className="text-destructive">{error}</p>}
    <section aria-label={node.name}>
      <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <div><dt className="text-xs text-muted-foreground">{t("proxyNodes.slug")}</dt><dd className="mt-1 font-mono text-[11px]">{node.slug}</dd></div>
        <div><dt className="text-xs text-muted-foreground">{t("proxyNodes.baseUrl")}</dt><dd className="mt-1 break-all font-mono text-[11px]">{node.base_url}</dd></div>
        <div><dt className="text-sm text-muted-foreground">{t("status")}</dt><dd className="mt-1"><StatusChip status={node.enabled ? "positive" : "neutral"} label={t(node.enabled ? "enabled" : "disabled")} /></dd></div>
        <div><dt className="text-sm text-muted-foreground">{t("proxyNodes.secret")}</dt><dd className="mt-1">{t("proxyNodes.secretVersion", { version: node.secret_version ?? 0 })}</dd></div>
        <div><dt className="text-sm text-muted-foreground">{t("proxyNodes.secretRotatedAt")}</dt><dd className="mt-1">{rotatedAt}</dd></div>
      </dl>
    </section>
    <AlertDialog open={deleteOpen} onOpenChange={setDeleteOpen}><AlertDialogContent className="surface" onCloseAutoFocus={event => {
      event.preventDefault();
      actionTrigger.current?.focus();
    }}>
      <AlertDialogHeader><AlertDialogTitle>{t("proxyNodes.deleteTitle")}</AlertDialogTitle><AlertDialogDescription>
        <Trans i18nKey="proxyNodes.deleteDescription" values={{ name: node.name }} components={{ 1: <strong /> }} />
      </AlertDialogDescription></AlertDialogHeader>
      <AlertDialogFooter><AlertDialogCancel>{t("cancel")}</AlertDialogCancel><AlertDialogAction onClick={() => mutation.mutate(node.id)} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">{t("delete")}</AlertDialogAction></AlertDialogFooter>
    </AlertDialogContent></AlertDialog>
  </div>;
}
