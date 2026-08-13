import { ProgressBar } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { loadingBar } from "./LoadingBar.recipe";

export type LoadingBarProps = {
  loading: boolean;
  label: string;
};

const LoadingBar: Layout<typeof loadingBar, LoadingBarProps> = () => (
  <ProgressBar
    {...slot.root}
    size="sm"
    color="accent"
    value={local.loading ? undefined : 0}
    isIndeterminate={local.loading}
    aria-hidden={!local.loading}
    aria-label={local.label}
  />
);

export const LoadingBarLayout = LoadingBar;
export default LoadingBar;
