import { Button, Flex, Text } from "@pathscale/ui";
import type { JSX } from "solid-js";
import { diagnostics, setDiagnostics } from "~/stores/diagnostics";
import { t } from "~/stores/i18n";

/**
 * The two diagnostics layers, in the same shape as the Mode row above them:
 * a label, what it costs you, and a pair of buttons.
 *
 * Two layers rather than one switch because they answer different questions.
 * Agent control is the browsing layer: read the page, click and type, which is what a
 * program driving this window as a browser needs, and nothing more. Deep
 * debugging is the other kind of question, why the window looks wrong: it adds
 * screenshots, layout and computed-style snapshots, renderer metrics and the
 * intrusive collectors. Splitting them means an agent using the browser is not
 * also paying for the machinery that explains it.
 *
 * Both are compiled into every build. Neither is a cargo feature, because a
 * capability that is absent rather than off is one that cannot be turned on at
 * the moment it is needed, which is always the moment something is already
 * wrong.
 *
 * A pair of buttons rather than one toggle because these are not cosmetic. "On"
 * for agent control means a socket any program running as you can drive this
 * window through, and a control whose current state you have to infer from a
 * highlight is the wrong control for that.
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
       * The second layer depends on the first: the runtime refuses to collect
       * while there is no control plane to read the samples back through, so
       * offering it as independently selectable would be offering a switch
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
          variant={props.value ? "solid" : "outline"}
          size="sm"
          aria-pressed={props.value}
          state={props.disabled ? "disabled" : undefined}
          onClick={() => props.onPick(true)}
        >
          {t("diagnostics.on")}
        </Button>
        <Button
          variant={props.value ? "outline" : "solid"}
          size="sm"
          aria-pressed={!props.value}
          state={props.disabled ? "disabled" : undefined}
          onClick={() => props.onPick(false)}
        >
          {t("diagnostics.off")}
        </Button>
      </Flex>
    </Flex>
  );
}
