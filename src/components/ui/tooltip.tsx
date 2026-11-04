// Adapted from shadcn/ui's Radix Tooltip.
import type { ReactElement } from "react";
import { Tooltip as Primitive } from "radix-ui";

export const TooltipProvider = Primitive.Provider;

export function Hint({
  label,
  children,
}: {
  label: string;
  children: ReactElement;
}) {
  return (
    <Primitive.Root>
      <Primitive.Trigger asChild>{children}</Primitive.Trigger>
      <Primitive.Portal>
        <Primitive.Content
          className="ui-tooltip"
          sideOffset={7}
          collisionPadding={8}
        >
          {label}
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
