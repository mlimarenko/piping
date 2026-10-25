import type { ComponentPropsWithoutRef, ComponentType } from "react";
import { TabsList, TabsTrigger } from "@/shared/ui-primitives/tabs";
import { cn } from "@/shared/lib/utils";

export interface SectionTabItem<TValue extends string = string> {
  value: TValue;
  label: string;
  icon?: ComponentType<{ className?: string }>;
}

/**
 * Standard `TabsList` attributes forwarded to the rendered tablist. `aria-label`
 * (or `aria-labelledby`) belongs here: the shared tablist otherwise has no
 * accessible name, and a tablist without one violates the tabs pattern.
 */
type SectionTabsListProps = Omit<
  ComponentPropsWithoutRef<typeof TabsList>,
  "children" | "className"
>;

interface SectionTabsProps<TValue extends string = string> extends SectionTabsListProps {
  items: readonly SectionTabItem<TValue>[];
  wrapperClassName?: string;
}

const SectionTabs = <TValue extends string = string>({
  items,
  wrapperClassName,
  ...listProps
}: SectionTabsProps<TValue>) => (
  <div className={cn("min-w-0 shrink-0", wrapperClassName)}>
    <TabsList {...listProps}>
      {items.map((item) => {
        const Icon = item.icon;
        return (
          <TabsTrigger
            key={item.value}
            value={item.value}
          >
            {Icon && <Icon className="size-4 shrink-0" aria-hidden />}
            {item.label}
          </TabsTrigger>
        );
      })}
    </TabsList>
  </div>
);

export default SectionTabs;
