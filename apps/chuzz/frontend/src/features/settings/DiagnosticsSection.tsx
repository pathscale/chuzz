import { Button, Flex, Text } from "@pathscale/ui";
import type { JSX } from "solid-js";
import { diagnostics, setDiagnostics } from "~/stores/diagnostics";
import { t } from "~/stores/i18n";

/**
 * The two diagnostics switches, in the same shape as the Mode row above them:
 * a label, what it costs you, and a pair of buttons.
 *
 * A pair rather than one toggle because these are not cosmetic. "On" for
 * inspection means a socket any local process can drive this window through,
 * and a control whose current state you have to infer from a highlight is the
 * wrong control for that.
 */
export function DiagnosticsSection(): JSX.Element {
  return (
    <Flex as="div" direction="col" gap="md">
      <Text size="sm" variant="muted">
        {t("diagnostics.title")}
      </Text>

      <Switch
        label={t("diagnostics.inspection")}
        hint={diagnostics.locked ? t("diagnostics.locked") : t("diagnostics.inspectionHint")}
        value={diagnostics.inspection}
        disabled={diagnostics.locked}
        onPick={(on) => void setDiagnostics(on, on && diagnostics.profiling)}
      />

      {/*
       * Deep profiling depends on inspection: the runtime refuses to collect
       * while there is no inspection plane to read the samples back through,
       * so offering it as independently selectable would be offering a switch
       * that does nothing.
       */}
      <Switch
        label={t("diagnostics.profiling")}
        hint={t("diagnostics.profilingHint")}
        value={diagnostics.profiling}
        disabled={diagnostics.locked || !diagnostics.inspection}
        onPick={(on) => void setDiagnostics(diagnostics.inspection, on)}
      />
    </Flex>
  );
}

function Switch(props: {
  label: string;
  hint: string;
  value: boolean;
  disabled: boolean;
  onPick: (on: boolean) => void;
}): JSX.Element {
  return (
    <Flex as="div" align="center" gap="md">
      <Flex as="div" direction="col" grow>
        <Text size="sm">{props.label}</Text>
        <Text size="xs" variant="muted">
          {props.hint}
        </Text>
      </Flex>
      <Flex as="div" align="center" gap="sm" shrink={false}>
        <Button
          variant={props.value ? "primary" : "outline"}
          size="sm"
          aria-pressed={props.value}
          isDisabled={props.disabled}
          onClick={() => props.onPick(true)}
        >
          {t("diagnostics.on")}
        </Button>
        <Button
          variant={props.value ? "outline" : "primary"}
          size="sm"
          aria-pressed={!props.value}
          isDisabled={props.disabled}
          onClick={() => props.onPick(false)}
        >
          {t("diagnostics.off")}
        </Button>
      </Flex>
    </Flex>
  );
}
