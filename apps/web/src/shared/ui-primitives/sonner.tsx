import { useTheme } from "@/shared/hooks/theme-context";
import { Toaster as Sonner, toast } from "sonner";

type ToasterProps = React.ComponentProps<typeof Sonner>;

const Toaster = ({ ...props }: ToasterProps) => {
  const { resolvedTheme } = useTheme();

  return (
    <Sonner
      theme={resolvedTheme}
      closeButton
      className="toaster group"
      toastOptions={{
        classNames: {
          toast:
            // Bodies pass through; only buttons on mounted, visible, live
            // front/expanded notices opt back into pointer interaction.
            "group toast !pointer-events-none [&[data-mounted=true][data-visible=true][data-removed=false][data-front=true]_button]:pointer-events-auto [&[data-mounted=true][data-visible=true][data-removed=false][data-expanded=true]_button]:pointer-events-auto group-[.toaster]:bg-background group-[.toaster]:text-foreground group-[.toaster]:border-border group-[.toaster]:shadow-lg group-[.toaster]:pr-10",
          description: "group-[.toast]:text-muted-foreground",
          actionButton: "group-[.toast]:bg-primary group-[.toast]:text-primary-foreground",
          cancelButton: "group-[.toast]:bg-muted group-[.toast]:text-muted-foreground",
          closeButton:
            "group-[.toaster]:border-border group-[.toaster]:bg-background group-[.toaster]:text-foreground group-[.toaster]:opacity-100 group-[.toaster]:shadow-sm",
        },
      }}
      {...props}
    />
  );
};

export { Toaster, toast };
