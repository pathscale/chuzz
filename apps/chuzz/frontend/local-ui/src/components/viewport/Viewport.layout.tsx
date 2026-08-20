import type { JSX } from "@solidjs/web";
import type { Layout } from "solid-layouts";
import { viewport } from "./Viewport.recipe";

export type ViewportProps = JSX.HTMLAttributes<HTMLDivElement>;

const Viewport: Layout<typeof viewport, ViewportProps> = () => <div {...slot.root}>{children}</div>;

export const ViewportLayout = Viewport;
export default Viewport;
