import * as SelectPrimitive from "@radix-ui/react-select";
import type { ReactNode } from "react";
import { Icon } from "./icons";

interface SelectOption {
  value: string;
  label: string;
  icon?: ReactNode;
}

export function Select({
  value,
  onValueChange,
  options,
  placeholder = "Choose an option",
  disabled = false,
  required = false,
  id,
  "aria-labelledby": labelledBy,
  "aria-describedby": describedBy,
}: {
  value: string;
  onValueChange: (value: string) => void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  required?: boolean;
  id?: string;
  "aria-labelledby"?: string;
  "aria-describedby"?: string;
}) {
  const selected = options.find((option) => option.value === value);
  function selectValue(next: string) {
    // Form synchronization can emit empty or unchanged values while options
    // mount. Only an actual choice should change the application's selection.
    if (next !== value && options.some((option) => option.value === next)) {
      onValueChange(next);
    }
  }
  return (
    <SelectPrimitive.Root
      value={selected?.value ?? ""}
      onValueChange={selectValue}
      disabled={disabled}
      required={required}
    >
      <SelectPrimitive.Trigger
        className="select-trigger"
        id={id}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
      >
        {selected?.icon && (
          <span className="select-option-icon" aria-hidden="true">
            {selected.icon}
          </span>
        )}
        <span className="select-value">
          <SelectPrimitive.Value placeholder={placeholder} />
        </span>
        <SelectPrimitive.Icon className="select-chevron">
          <Icon name="chevron" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          className="select-menu"
          position="popper"
          sideOffset={6}
          collisionPadding={12}
        >
          <SelectPrimitive.ScrollUpButton className="select-scroll select-scroll-up">
            <Icon name="chevron" />
          </SelectPrimitive.ScrollUpButton>
          <SelectPrimitive.Viewport className="select-viewport">
            {options.map((option) => (
              <SelectPrimitive.Item
                className="select-option"
                key={option.value}
                value={option.value}
                textValue={option.label}
              >
                {option.icon && (
                  <span className="select-option-icon" aria-hidden="true">
                    {option.icon}
                  </span>
                )}
                <SelectPrimitive.ItemText>
                  {option.label}
                </SelectPrimitive.ItemText>
                <SelectPrimitive.ItemIndicator className="select-check">
                  <Icon name="check" />
                </SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
          <SelectPrimitive.ScrollDownButton className="select-scroll">
            <Icon name="chevron" />
          </SelectPrimitive.ScrollDownButton>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
