import { Input } from "@pathscale/ui";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { addressBar } from "./AddressBar.recipe";

export type AddressBarProps = {
  value: string;
  invalid: boolean;
  placeholder: string;
  onInput: JSX.EventHandlerUnion<HTMLInputElement, InputEvent>;
  onKeyDown: JSX.EventHandlerUnion<HTMLInputElement, KeyboardEvent>;
};

const AddressBar: Layout<typeof addressBar, AddressBarProps> = () => (
  <Input
    {...slot.root}
    type="text"
    value={local.value}
    isInvalid={local.invalid}
    fullWidth
    placeholder={local.placeholder}
    onInput={local.onInput}
    onKeyDown={local.onKeyDown}
  />
);

export const AddressBarLayout = AddressBar;
export default AddressBar;
