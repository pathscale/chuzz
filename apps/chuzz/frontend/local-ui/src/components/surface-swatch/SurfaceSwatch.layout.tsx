import { ColorSwatch } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { surfaceSwatch } from "./SurfaceSwatch.recipe";

export type SurfaceSwatchProps = {
  color: string;
  label: string;
  x: number;
  y: number;
};

const SurfaceSwatch: Layout<typeof surfaceSwatch, SurfaceSwatchProps> = () => (
  <ColorSwatch
    color={local.color}
    colorName={local.label}
    size="md"
    title={local.color}
    style={{
      left: `calc(50% + ${local.x.toFixed(2)}px)`,
      top: `calc(50% + ${local.y.toFixed(2)}px)`,
    }}
    {...slot.root}
  />
);

export const SurfaceSwatchLayout = SurfaceSwatch;
export default SurfaceSwatch;
