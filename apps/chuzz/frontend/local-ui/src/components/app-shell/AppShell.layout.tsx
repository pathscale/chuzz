import "../../styles.css";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { appShell } from "./AppShell.recipe";

export type AppShellProps = JSX.HTMLAttributes<HTMLDivElement>;

const AppShell: Layout<typeof appShell, AppShellProps> = () => (
  <div {...slot.root}>{children}</div>
);

export const AppShellLayout = AppShell;
export default AppShell;
