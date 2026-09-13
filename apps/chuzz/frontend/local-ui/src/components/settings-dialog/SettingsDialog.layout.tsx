import { Button, Dialog, Text } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
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
  <Dialog
    id="chuzz-settings-dialog"
    {...slot.root}
    open={local.open}
    onOpenChange={(open) => {
      if (!open) local.onClose();
    }}
    placement="center"
    size="lg"
    backdrop="opaque"
  >
    <Dialog.Content id="chuzz-settings-content" {...slot.content}>
      <Dialog.Header {...slot.header}>
        <Dialog.Heading {...slot.title}>{local.title}</Dialog.Heading>
        <Dialog.CloseTrigger
          id="chuzz-settings-close"
          aria-label={`Close ${local.title}`}
          onClick={local.onClose}
        />
      </Dialog.Header>
      <Dialog.Body {...slot.body}>
        <div {...slot.row}>
          <Text size="sm" variant="muted" {...slot.label}>
            {local.modeLabel}
          </Text>
          <div {...slot.modeGroup}>
            <Button
              id="chuzz-theme-dark"
              variant={local.mode === "dark" ? "solid" : "outline"}
              size="sm"
              aria-pressed={local.mode === "dark" ? "true" : "false"}
              onClick={() => local.onModeChange("dark")}
            >
              {local.darkLabel}
            </Button>
            <Button
              id="chuzz-theme-light"
              variant={local.mode === "light" ? "solid" : "outline"}
              size="sm"
              aria-pressed={local.mode === "light" ? "true" : "false"}
              onClick={() => local.onModeChange("light")}
            >
              {local.lightLabel}
            </Button>
          </div>
        </div>
        {children}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog>
);

export const SettingsDialogLayout = SettingsDialog;
export default SettingsDialog;
