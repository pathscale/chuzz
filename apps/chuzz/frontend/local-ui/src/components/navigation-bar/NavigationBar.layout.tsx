import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { navigationBar } from "./NavigationBar.recipe";

export type NavigationBarProps = JSX.HTMLAttributes<HTMLDivElement>;

const NavigationBar: Layout<typeof navigationBar, NavigationBarProps> = () => (
  <div {...slot.root}>{children}</div>
);

export const NavigationBarLayout = NavigationBar;
export default NavigationBar;
