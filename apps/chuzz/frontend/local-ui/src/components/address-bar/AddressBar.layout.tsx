import { Input } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { addressBar } from "./AddressBar.recipe";

export type AddressBarProps = {
  id: string;
  value: string;
  invalid: boolean;
  placeholder: string;
  onInput: JSX.EventHandlerUnion<HTMLInputElement, InputEvent>;
};

/**
 * Deliberately unsized.
 *
 * `Input` used to default to `md` and now defaults to `sm`, which is 4px
 * shorter. It makes no difference here: `.navigation-bar .input-control` pins
 * the field at 32px with its own padding, radius and font size, and outranks
 * the component's size class on specificity. Passing `size="md"` back would be
 * naming a value nothing reads, so the height stays where the chrome's own
 * stylesheet puts it.
 */
const AddressBar: Layout<typeof addressBar, AddressBarProps> = () => (
  <Input
    {...slot.root}
    id={local.id}
    type="text"
    value={local.value}
    issues={local.invalid ? [{ code: "invalid-url" }] : undefined}
    fullWidth
    placeholder={local.placeholder}
    onInput={local.onInput}
  />
);

export const AddressBarLayout = AddressBar;
export default AddressBar;
