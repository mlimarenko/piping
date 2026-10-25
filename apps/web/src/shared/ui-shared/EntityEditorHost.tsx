import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import EntityEditorPage from "./EntityEditorPage";
import PageLayout from "./PageLayout";
import { usePageActionsBlocker } from "./pageChromeContext";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
  footer: ReactNode;
  children: ReactNode;
  dirty?: boolean;
  saving?: boolean;
  className?: string;
}

/** Hosts a domain-owned form as a page without unmounting its source workspace.
 * Routed editors render EntityEditorPage directly; embedded editors use this
 * viewport as their scroll boundary while retaining the source workspace.
 */
export default function EntityEditorHost({ open, onOpenChange, title, description, footer, children, dirty, saving, className }: Props) {
  const { t } = useTranslation();
  const id = useId();
  const pageRef = useRef<HTMLDivElement>(null);
  const [invoker, setInvoker] = useState<Element | null>(() => document.activeElement);
  usePageActionsBlocker(id, open, invoker);
  const [changed, setChanged] = useState(false);
  const [previousOpen, setPreviousOpen] = useState(open);
  if (open !== previousOpen) {
    // Capture before portal children with autoFocus mount and take focus.
    if (open) setInvoker(document.activeElement);
    setPreviousOpen(open);
    setChanged(false);
  }
  const target = document.getElementById("main-content") ?? document.body;
  useEffect(() => {
    if (!open) return;
    const siblings = [...target.children].filter(node => node !== pageRef.current && node instanceof HTMLElement) as HTMLElement[];
    const previous = siblings.map(node => ({ node, inert: node.hasAttribute("inert"), hidden: node.getAttribute("aria-hidden") }));
    for (const node of siblings) { node.setAttribute("inert", ""); node.setAttribute("aria-hidden", "true"); }
    pageRef.current?.querySelector<HTMLElement>("h2")?.focus();
    return () => {
      for (const { node, inert, hidden } of previous) {
        node.toggleAttribute("inert", inert);
        if (hidden === null) node.removeAttribute("aria-hidden"); else node.setAttribute("aria-hidden", hidden);
      }
    };
  }, [open, target, invoker]);
  if (!open) return null;
  return createPortal(
    <div ref={pageRef} data-entity-editor-host={id} className="absolute inset-0 z-30 min-h-0 min-w-0 overflow-y-auto overflow-x-hidden overscroll-y-contain bg-background"
      onInputCapture={() => setChanged(true)} onChangeCapture={() => setChanged(true)}
      onClickCapture={event => {
        // Toggle-style controls that own their state via aria-pressed (e.g.
        // TogglePill) emit no input/change event, so recognise them too.
        if ((event.target as HTMLElement).closest('[role="switch"], [role="checkbox"], [role="option"], [aria-pressed]')) setChanged(true);
      }}>
      <PageLayout fill><EntityEditorPage scope="embedded" title={title} description={description} onBack={() => onOpenChange(false)} backLabel={t("back")}
        footer={footer} dirty={dirty ?? changed} saving={saving} dirtyMessage={t("editor.unsavedChanges")} className={className}>
        {children}
      </EntityEditorPage></PageLayout>
    </div>, target,
  );
}
