import { Button, Modal, Text } from "@pathscale/ui";
import type { JSX } from "solid-js";
import type { Layout } from "solid-layouts";
import { settingsDialog } from "./SettingsDialog.recipe";

export type SettingsDialogProps = {
  children: JSX.Element;
  open: boolean;
  onClose: () => void;
  title: string;
  modeLabel: string;
  darkLabel: string;
  lightLabel: string;
  mode: "dark" | "light";
  onModeChange: (mode: "dark" | "light") => void;
};

const SettingsDialog: Layout<typeof settingsDialog, SettingsDialogProps> = () => (
  <Modal
    {...slot.root}
    isOpen={local.open}
    onOpenChange={(open) => {
      if (!open) local.onClose();
    }}
    placement="center"
    size="lg"
    backdrop="opaque"
  >
    <Modal.Content {...slot.content}>
      <Modal.Header {...slot.header}>
        <Modal.Heading {...slot.title}>{local.title}</Modal.Heading>
        <Modal.CloseTrigger aria-label={local.title} />
      </Modal.Header>
      <Modal.Body {...slot.body}>
        <div {...slot.row}>
          <Text size="sm" variant="muted" {...slot.label}>
            {local.modeLabel}
          </Text>
          <div {...slot.modeGroup}>
            <Button
              variant={local.mode === "dark" ? "primary" : "outline"}
              size="sm"
              aria-pressed={local.mode === "dark"}
              onClick={() => local.onModeChange("dark")}
            >
              {local.darkLabel}
            </Button>
            <Button
              variant={local.mode === "light" ? "primary" : "outline"}
              size="sm"
              aria-pressed={local.mode === "light"}
              onClick={() => local.onModeChange("light")}
            >
              {local.lightLabel}
            </Button>
          </div>
        </div>
        {children}
      </Modal.Body>
    </Modal.Content>
  </Modal>
);

export const SettingsDialogLayout = SettingsDialog;
export default SettingsDialog;
