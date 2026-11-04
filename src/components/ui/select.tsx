// Adapted from shadcn/ui's Radix Select, with ethos material styles.
import type { ComponentProps } from "react";
import { Select as Primitive } from "radix-ui";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { clsx } from "clsx";

export const Select = Primitive.Root;
export const SelectValue = Primitive.Value;

export function SelectTrigger({
  className,
  children,
  ...props
}: ComponentProps<typeof Primitive.Trigger>) {
  return (
    <Primitive.Trigger
      data-slot="select-trigger"
      className={clsx("ui-select-trigger", className)}
      {...props}
    >
      {children}
      <Primitive.Icon asChild>
        <ChevronDown size={14} />
      </Primitive.Icon>
    </Primitive.Trigger>
  );
}

export function SelectContent({
  className,
  children,
  ...props
}: ComponentProps<typeof Primitive.Content>) {
  return (
    <Primitive.Portal>
      <Primitive.Content
        data-slot="select-content"
        className={clsx("ui-select-content", className)}
        position="popper"
        sideOffset={6}
        collisionPadding={12}
        {...props}
      >
        <Primitive.ScrollUpButton className="ui-select-scroll">
          <ChevronUp size={14} />
        </Primitive.ScrollUpButton>
        <Primitive.Viewport className="ui-select-viewport">
          {children}
        </Primitive.Viewport>
        <Primitive.ScrollDownButton className="ui-select-scroll">
          <ChevronDown size={14} />
        </Primitive.ScrollDownButton>
      </Primitive.Content>
    </Primitive.Portal>
  );
}

export function SelectItem({
  className,
  children,
  ...props
}: ComponentProps<typeof Primitive.Item>) {
  return (
    <Primitive.Item
      data-slot="select-item"
      className={clsx("ui-select-item", className)}
      {...props}
    >
      <Primitive.ItemText>{children}</Primitive.ItemText>
      <Primitive.ItemIndicator className="ui-select-check">
        <Check size={14} />
      </Primitive.ItemIndicator>
    </Primitive.Item>
  );
}
