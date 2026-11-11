import type { ComponentProps } from "react";
import { Layers } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import {
  ENTITY_ICONS,
  isEntityIconName,
  isSafeIconDataUrl,
  type EntityIconName,
} from "@/shared/entity-icon-utils";

interface EntityIconProps extends Omit<ComponentProps<"span">, "children"> {
  iconName?: string | null;
  customDataUrl?: string | null;
  fallback: EntityIconName;
  iconClassName?: string;
}

export function EntityIcon({
  iconName,
  customDataUrl,
  fallback,
  className,
  iconClassName,
  ...props
}: EntityIconProps) {
  if (isSafeIconDataUrl(customDataUrl)) {
    return (
      <span className={cn("inline-flex shrink-0 items-center justify-center overflow-hidden", className)} {...props}>
        <img src={customDataUrl} alt="" className={cn("h-full w-full object-contain", iconClassName)} />
      </span>
    );
  }

  const resolved = isEntityIconName(iconName) ? iconName : fallback;
  const Icon = ENTITY_ICONS[resolved] ?? Layers;
  return (
    <span className={cn("inline-flex shrink-0 items-center justify-center", className)} {...props}>
      <Icon className={iconClassName} />
    </span>
  );
}
