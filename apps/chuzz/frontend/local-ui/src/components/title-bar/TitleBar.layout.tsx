import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { titleBar } from "./TitleBar.recipe";

export type TitleBarProps = JSX.HTMLAttributes<HTMLDivElement>;

const TitleBar: Layout<typeof titleBar, TitleBarProps> = () => <div {...slot.root}>{children}</div>;

export const TitleBarLayout = TitleBar;
export default TitleBar;
