import type { HTMLAttributes } from "react";
import { cn } from "@/shared/lib/utils";

interface PageLayoutProps extends HTMLAttributes<HTMLDivElement> {
  fill?: boolean;
}

/** The management route and retained editor host each apply this canvas once. */
export default function PageLayout({
  fill = false, className, children, ...props
}: PageLayoutProps) {
  return <div data-slot="page-layout" className={cn(
    "page-gutters min-w-0 w-full py-4 md:py-6",
    fill && "min-h-full",
    className,
  )} {...props}>{children}</div>;
}
