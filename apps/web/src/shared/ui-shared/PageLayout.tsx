import type { HTMLAttributes } from "react";
import { cn } from "@/shared/lib/utils";

interface PageLayoutProps extends HTMLAttributes<HTMLDivElement> {
  width?: "standard" | "form" | "compact" | "full";
  inset?: boolean;
  fill?: boolean;
}

/** Width and gutters are shared by routed pages and embedded editor shells. */
export default function PageLayout({
  width = "standard", inset = true, fill = false, className, children, ...props
}: PageLayoutProps) {
  const widths = {
    standard: "max-w-[1280px]",
    form: "max-w-[960px]",
    compact: "max-w-[800px]",
    full: "max-w-none",
  };
  return <div data-slot="page-layout" className={cn(
    "mx-auto min-w-0 w-full", widths[width],
    inset && "px-4 py-4 md:px-6 md:py-6",
    fill && "h-full min-h-0",
    className,
  )} {...props}>{children}</div>;
}
