import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";
import type { ReactNode } from "react";

export function Tooltip({
  children,
  text,
  disabled = false,
  className,
}: {
  children: ReactNode;
  text: string;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <BaseTooltip.Provider delay={250}>
      <BaseTooltip.Root>
        <BaseTooltip.Trigger
          render={
            <span className={className} tabIndex={disabled ? 0 : undefined} />
          }
        >
          {children}
        </BaseTooltip.Trigger>
        <BaseTooltip.Portal>
          <BaseTooltip.Positioner sideOffset={6} className="z-50">
            <BaseTooltip.Popup className="max-w-64 rounded-md bg-foreground px-3 py-2 text-xs text-background shadow-lg">
              {text}
            </BaseTooltip.Popup>
          </BaseTooltip.Positioner>
        </BaseTooltip.Portal>
      </BaseTooltip.Root>
    </BaseTooltip.Provider>
  );
}
