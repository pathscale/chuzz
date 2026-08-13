import { SurfaceSwatch, SurfaceWheel } from "@chuzz/ui";
import { Button, ColorSwatch, Flex, Text } from "@pathscale/ui";
import { createEffect, For, type JSX, Show } from "solid-js";
import {
  accentOptions,
  BRIGHTNESS_STOPS,
  closestColorIndex,
  DEFAULT_ACCENT,
  MAX_SOFTNESS,
  normalizeWash,
  surfaceColors,
  WASH_STOPS,
} from "~/lib/theme";
import { t } from "~/stores/i18n";
import { prefs } from "~/stores/prefs";
import type { ThemeSettings } from "~/types";

/**
 * The colour wheel and the softness strip, side by side.
 *
 * The wheel owns literal AgencyZero colours rather than borrowing the
 * upstream contrast palette. That palette makes dark-mode swatches pastel so
 * they stand out from a black ring; here the swatch is the value, so dark mode
 * must offer dark-oriented colours and a pressed dot must equal the stored hex.
 *
 * The strip beside it is ours. nofilter's greyscale column switches between two
 * themes; there is one theme here, so the column drives the axis that actually
 * helps — how far the surfaces lift off the near-black floor.
 */
export function ThemePicker(props: {
  theme: ThemeSettings;
  onSurface: (hex: string) => void;
  onAccent: (hex: string) => void;
  onSoftness: (value: number) => void;
  onWash: (value: number) => void;
  onBrightness: (value: number) => void;
  onReset: () => void;
  isDefault: boolean;
}): JSX.Element {
  /** Five stops across the comfort range, matching the strength row beside it. */
  const softnessStops = () => Array.from({ length: 5 }, (_, i) => (i * MAX_SOFTNESS) / 4);

  /** The desk as currently configured — what every swatch sits on. */
  const deskAnchor = (softness: number) =>
    prefs.colorMode === "light"
      ? `oklch(calc(93% - ${softness}%) 0.004 240)`
      : `oklch(calc(10.5% + ${softness}%) 0.004 240)`;
  const deskStrength = (wash: number) => Math.min(wash * 1.1, 100);
  const deskPreview = (theme: ThemeSettings) =>
    `color-mix(in oklab, ${theme.surface || DEFAULT_ACCENT} ${deskStrength(theme.wash)}%, ${deskAnchor(theme.softness)})`;

  return (
    <Flex as="div" align="start" gap="md" paddingInline="md" paddingBlock="md">
      <Flex as="div" shrink={false}>
        <SurfaceColorWheel value={props.theme.surface} onPick={props.onSurface} />
      </Flex>

      <Flex as="div" direction="col" gap="md" grow>
        {/*
         * Strength before softness: it is the one that decides whether the
         * wheel did anything at all, and each swatch previews the desk it
         * produces so the row reads as its own effect rather than as five
         * circles. At 0 the workspace stays the designed grey and only the
         * accent moves — which is a legitimate choice, just not the default.
         */}
        <Axis
          label={t("appearance.colourStrength")}
          hint={t("appearance.colourStrengthHint")}
          stops={[...WASH_STOPS]}
          value={normalizeWash(props.theme.wash)}
          onPick={props.onWash}
          preview={(stop) =>
            `color-mix(in oklab, ${props.theme.surface || DEFAULT_ACCENT} ${deskStrength(stop)}%, ${deskAnchor(props.theme.softness)})`
          }
          format={(stop) => `${stop}%`}
          action={
            <Button
              variant="outline"
              size="sm"
              aria-label={t("appearance.resetButton")}
              isDisabled={props.isDefault}
              onClick={props.onReset}
            >
              {t("appearance.resetButton")}
            </Button>
          }
        />

        <Axis
          label={t("appearance.softness")}
          hint={t("appearance.softnessHint")}
          stops={softnessStops()}
          value={props.theme.softness}
          onPick={props.onSoftness}
          preview={(stop) =>
            `color-mix(in oklab, ${props.theme.surface || DEFAULT_ACCENT} ${deskStrength(props.theme.wash)}%, ${deskAnchor(stop)})`
          }
          format={(stop) => `${Math.round((stop / MAX_SOFTNESS) * 100)}%`}
        />

        {/*
         * The rungs are the point here, so these swatches carry a letter in
         * the text colour they produce rather than showing the colour as a
         * fill. A row of five near-identical pale circles says nothing; five
         * letters at different weights is the actual question being asked.
         */}
        <Axis
          label={t("appearance.textBrightness")}
          hint={t("appearance.textBrightnessHint")}
          stops={[...BRIGHTNESS_STOPS]}
          value={props.theme.textBrightness}
          onPick={props.onBrightness}
          preview={() => deskPreview(props.theme)}
          ink={(stop) =>
            prefs.colorMode === "light"
              ? `oklch(calc(28% + ${props.theme.softness * 0.45 - stop}%) 0.009 245)`
              : `oklch(calc(75% - ${props.theme.softness * 0.45 - stop}%) 0.009 245)`
          }
          format={(_stop, index) => `${Math.round((index / (BRIGHTNESS_STOPS.length - 1)) * 100)}%`}
        />

        <AccentSelector
          surface={props.theme.surface || DEFAULT_ACCENT}
          accent={props.theme.accent}
          wash={props.theme.wash}
          softness={props.theme.softness}
          onPick={props.onAccent}
        />
      </Flex>
    </Flex>
  );
}

/** Concentric literal swatches: what is shown is exactly what gets persisted. */
function SurfaceColorWheel(props: { value: string; onPick: (value: string) => void }): JSX.Element {
  const layout = [
    ...Array.from({ length: 12 }, (_, index) => ({ count: 12, index, radius: 56, phase: -30 })),
    ...Array.from({ length: 12 }, (_, index) => ({ count: 12, index, radius: 40, phase: -30 })),
    ...Array.from({ length: 6 }, (_, index) => ({ count: 6, index, radius: 21, phase: -60 })),
    { count: 1, index: 0, radius: 0, phase: 0 },
  ];
  const colors = () => surfaceColors(prefs.colorMode);
  let previous = colors();

  // Preserve the same petal across a mode change, and migrate the upstream
  // palette values written by earlier builds to the nearest literal petal.
  createEffect(() => {
    const next = colors();
    const value = props.value.trim().toLowerCase();
    if (value) {
      let selected = previous.findIndex((color) => color.toLowerCase() === value);
      if (selected < 0) selected = closestColorIndex(value, previous);
      const rebased = next[selected];
      if (rebased && rebased.toLowerCase() !== value) props.onPick(rebased);
    }
    previous = next;
  });

  return (
    <SurfaceWheel
      value={props.value}
      onChange={props.onPick}
      label={t("appearance.surfaceColour")}
    >
      <For each={colors()}>
        {(color, index) => {
          const point = layout[index()];
          const angle = ((point.index / point.count) * 360 + point.phase) * (Math.PI / 180);
          const x = Math.cos(angle) * point.radius;
          const y = Math.sin(angle) * point.radius;
          return (
            <SurfaceSwatch
              color={color}
              label={`${t("appearance.surfaceColour")} ${color}`}
              x={x}
              y={y}
            />
          );
        }}
      </For>
    </SurfaceWheel>
  );
}

/** An independent high-contrast colour for controls, rings and active states. */
function AccentSelector(props: {
  surface: string;
  accent: string;
  wash: number;
  softness: number;
  onPick: (value: string) => void;
}): JSX.Element {
  const options = () => accentOptions(props.surface, prefs.colorMode, props.wash, props.softness);
  let previous = options();

  // A palette choice is semantic, not a frozen hex. Keep the same harmony
  // selected while its rendered colour responds to surface, mode, strength,
  // and softness. Older arbitrary hex values migrate to the nearest harmony.
  createEffect(() => {
    const next = options();
    let selected = previous.findIndex((option) => option.value === props.accent);
    if (selected < 1 && props.accent) {
      const closest = closestColorIndex(
        props.accent,
        previous.slice(1).map((option) => option.color),
      );
      if (closest >= 0) selected = closest + 1;
    }
    if (selected > 0 && next[selected]?.value !== props.accent) {
      props.onPick(next[selected].value);
    }
    previous = next;
  });
  return (
    <Flex as="div" direction="col" gap="sm">
      <Flex as="div" align="baseline" gap="sm">
        <Text
          size="xs"
          variant="muted"
          weight="semibold"
          transform="uppercase"
          tracking="wide"
        >
          {t("appearance.accentColour")}
        </Text>
        <Text size="xs" variant="subtle">
          {t("appearance.accentColourHint")}
        </Text>
      </Flex>
      <Flex as="div" align="center" gap="sm">
        <For each={options()}>
          {(option, index) => {
            const selected = () => props.accent === option.value;
            const label = () =>
              index() === 0
                ? t("appearance.designedYellow")
                : `${t("appearance.accentColour")} ${index() + 1}`;
            return (
              <ColorSwatch
                color={option.color}
                colorName={label()}
                size="md"
                isSelected={selected()}
                title={label()}
                onSelect={() => props.onPick(option.value)}
              />
            );
          }}
        </For>
      </Flex>
    </Flex>
  );
}

/**
 * One row of preview swatches.
 *
 * Horizontal rather than the vertical column nofilter uses: theirs picks one of
 * six greyscale *themes*, ours moves a continuum, and a row reads as a slider
 * where a column reads as a menu.
 */
function Axis(props: {
  label: string;
  hint: string;
  stops: number[];
  value: number;
  onPick: (value: number) => void;
  preview: (stop: number) => string;
  /** When present, the swatch shows a letter in this colour instead of a fill alone. */
  ink?: (stop: number) => string;
  format: (stop: number, index: number) => string;
  action?: JSX.Element;
}): JSX.Element {
  const selected = (stop: number) => Math.abs(props.value - stop) < 0.01;
  return (
    <Flex as="div" direction="col" gap="sm">
      <Flex as="div" align="baseline" gap="sm">
        <Flex as="span" align="baseline" gap="sm" grow>
          <Text
            size="xs"
            variant="muted"
            weight="semibold"
            transform="uppercase"
            tracking="wide"
          >
            {props.label}
          </Text>
          <Text size="xs" variant="subtle">
            {props.hint}
          </Text>
        </Flex>
        {props.action}
      </Flex>
      <Flex as="div" align="center" gap="sm">
        <For each={props.stops}>
          {(stop, index) => (
            <Button
              variant={selected(stop) ? "primary" : "outline"}
              size="sm"
              isIconOnly
              aria-label={`${props.label} ${props.format(stop, index())}`}
              aria-pressed={selected(stop)}
              onClick={() => props.onPick(stop)}
              style={{ "background-color": props.preview(stop) }}
            >
              <Show when={props.ink}>
                {(ink) => (
                  <Text
                    size="xs"
                    weight="semibold"
                    leading="none"
                    style={{ color: ink()(stop) }}
                  >
                    A
                  </Text>
                )}
              </Show>
            </Button>
          )}
        </For>
      </Flex>
    </Flex>
  );
}
