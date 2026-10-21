import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/shared/ui-primitives/sheet";
import type { Dataflow } from "./dataflows-api";
import { FlowStatus, FlowTraffic } from "./DataflowConnection";
import DataflowEndpointMembers from "./DataflowEndpointMembers";

export default function DataflowDetail({ flow, onClose, onCloseAutoFocus, renderActions, writeFailed }: Readonly<{
  flow?: Dataflow; onClose: () => void; onCloseAutoFocus: (event: Event) => void; renderActions?: (flow: Dataflow) => ReactNode; writeFailed: boolean;
}>) {
  const { t } = useTranslation();
  return <Sheet open={!!flow} onOpenChange={open => { if (!open) onClose(); }}>
    <SheetContent className="flex w-full flex-col gap-6 overflow-y-auto text-foreground sm:max-w-xl" onCloseAutoFocus={onCloseAutoFocus}>
      {flow && <>
        <SheetHeader><SheetTitle className="break-words">{flow.name}</SheetTitle><SheetDescription>{t("dataflows.detailHint")}</SheetDescription></SheetHeader>
        <div className="flex flex-wrap items-center gap-2"><FlowStatus flow={flow} /><FlowTraffic flow={flow} />{renderActions?.(flow)}</div>
        {writeFailed && <p role="alert" className="text-sm text-destructive">{t("dataflows.writeFailed")}</p>}
        <DataflowEndpointMembers key={`source:${flow.id}`} value={flow.source} label={t("dataflows.source")} />
        <DataflowEndpointMembers key={`destination:${flow.id}`} value={flow.destination} label={t("dataflows.destination")} />
        {flow.description && <details className="border-t border-border pt-4"><summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("dataflows.evidence")}</summary><p className="mt-4 whitespace-pre-wrap break-words text-sm text-muted-foreground">{flow.description}</p></details>}
      </>}
    </SheetContent>
  </Sheet>;
}
