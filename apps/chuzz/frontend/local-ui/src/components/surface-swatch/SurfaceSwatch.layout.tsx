import { ColorSwatch } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { surfaceSwatch } from "./SurfaceSwatch.recipe";

export type SurfaceSwatchProps = {
  controlId: string;
  color: string;
  label: string;
  selected: boolean;
  x: number;
  y: number;
  onSelect: () => void;
};

const SurfaceSwatch: Layout<typeof surfaceSwatch, SurfaceSwatchProps> = () => (
  <span
    {...slot.root}
    style={{
      left: `calc(50% + ${local.x.toFixed(2)}px)`,
      top: `calc(50% + ${local.y.toFixed(2)}px)`,
    }}
  >
    <ColorSwatch
      id={local.controlId}
      color={local.color}
      colorName={local.label}
      size="md"
      isSelected={local.selected}
      title={local.color}
      onSelect={local.onSelect}
    />
  </span>
);

export const SurfaceSwatchLayout = SurfaceSwatch;
export default SurfaceSwatch;
