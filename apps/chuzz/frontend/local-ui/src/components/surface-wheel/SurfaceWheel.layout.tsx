import { ColorSwatchPicker } from "@pathscale/ui/lab";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { surfaceWheel } from "./SurfaceWheel.recipe";

export type SurfaceWheelProps = {
  children: JSX.Element;
  value: string;
  onChange: (value: string) => void;
  label: string;
};

const SurfaceWheel: Layout<typeof surfaceWheel, SurfaceWheelProps> = () => (
  <ColorSwatchPicker
    value={local.value}
    onChange={local.onChange}
    aria-label={local.label}
    {...slot.root}
  >
    {children}
  </ColorSwatchPicker>
);

export const SurfaceWheelLayout = SurfaceWheel;
export default SurfaceWheel;
