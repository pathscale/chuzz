import { Input } from "@pathscale/ui";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { addressBar } from "./AddressBar.recipe";

export type AddressBarProps = {
  id: string;
  value: string;
  invalid: boolean;
  placeholder: string;
  onInput: JSX.EventHandlerUnion<HTMLInputElement, InputEvent>;
};

const AddressBar: Layout<typeof addressBar, AddressBarProps> = () => (
  <Input
    {...slot.root}
    id={local.id}
    type="text"
    value={local.value}
    isInvalid={local.invalid}
    fullWidth
    placeholder={local.placeholder}
    onInput={local.onInput}
  />
);

export const AddressBarLayout = AddressBar;
export default AddressBar;
