import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { mainContent } from "./MainContent.recipe";

export type MainContentProps = JSX.HTMLAttributes<HTMLDivElement>;

const MainContent: Layout<typeof mainContent, MainContentProps> = () => (
  <div {...slot.root}>{children}</div>
);

export const MainContentLayout = MainContent;
export default MainContent;
