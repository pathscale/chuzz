import { Avatar } from "@pathscale/ui";
import type { Layout } from "solid-layouts";
import { avatar } from "./Avatar.recipe";

export type AvatarProps = {
  label: string;
};

const Avatar: Layout<typeof avatar, AvatarProps> = () => (
  <Avatar size="sm" variant="soft" {...slot.root}>
    <Avatar.Fallback>{local.label}</Avatar.Fallback>
  </Avatar>
);

export const AvatarLayout = Avatar;
export default Avatar;
