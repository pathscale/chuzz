import { Avatar as PathscaleAvatar } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { avatar } from "./Avatar.recipe";

export type AvatarProps = {
  label: string;
};

const Avatar: Layout<typeof avatar, AvatarProps> = () => (
  <PathscaleAvatar size="sm" variant="soft" {...slot.root}>
    <PathscaleAvatar.Fallback>{local.label}</PathscaleAvatar.Fallback>
  </PathscaleAvatar>
);

export const AvatarLayout = Avatar;
export default Avatar;
