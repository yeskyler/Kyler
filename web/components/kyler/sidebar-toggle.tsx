import type { ComponentProps } from "react";
import { PanelLeftIcon } from "lucide-react";
import { cn } from "cn";

type SidebarToggleProps = ComponentProps<"button"> & {
  direction: "collapse" | "expand";
};

export function SidebarToggle({ direction, className, ...props }: SidebarToggleProps) {
  const label = direction === "collapse" ? "Collapse sidebar" : "Expand sidebar";
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex size-7 shrink-0 items-center justify-center rounded-md border-0 bg-transparent p-0 text-muted-foreground/70 outline-none transition-colors hover:bg-foreground/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 dark:hover:bg-foreground/10 motion-reduce:transition-none",
        className,
      )}
      {...props}
    >
      <PanelLeftIcon aria-hidden className="size-[18px]" strokeWidth={1.5} />
    </button>
  );
}
