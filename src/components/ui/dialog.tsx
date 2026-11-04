// Adapted from shadcn/ui's Radix Dialog.
import type { ComponentProps } from "react";
import { Dialog as Primitive } from "radix-ui";
import { clsx } from "clsx";

export const Dialog = Primitive.Root;
export const DialogTitle = Primitive.Title;
export const DialogDescription = Primitive.Description;
export const DialogClose = Primitive.Close;

export function DialogContent({
  className,
  children,
  ...props
}: ComponentProps<typeof Primitive.Content>) {
  return (
    <Primitive.Portal>
      <Primitive.Overlay
        data-slot="dialog-overlay"
        className="ui-dialog-overlay"
      />
      <Primitive.Content
        data-slot="dialog-content"
        className={clsx("ui-dialog-content", className)}
        {...props}
      >
        {children}
      </Primitive.Content>
    </Primitive.Portal>
  );
}
