import { Button, Flex, Text } from "@pathscale/ui";
import type { JSX } from "@solidjs/web";
import { t } from "~/stores/i18n";
import { setUserAgentSpoofing, userAgent } from "~/stores/user-agent";

export function BrowserIdentitySection(): JSX.Element {
  return (
    <Flex as="div" direction="col" gap="sm">
      <Text size="sm" variant="muted">
        {t("identity.title")}
      </Text>
      <Flex as="div" align="center" gap="md">
        <Flex as="div" direction="col" grow>
          <Text size="sm">{t("identity.label")}</Text>
          <Text size="xs" variant="muted">
            {userAgent.locked ? t("identity.locked") : t("identity.hint")}
          </Text>
        </Flex>
        <Flex as="div" align="center" gap="sm" shrink={false}>
          <Button
            id="chuzz-user-agent-spoof-on"
            variant={userAgent.spoofing ? "solid" : "outline"}
            size="sm"
            aria-label={`${t("identity.label")} ${t("identity.on")}`}
            aria-pressed={userAgent.spoofing ? "true" : "false"}
            state={userAgent.locked ? "disabled" : undefined}
            onClick={() => void setUserAgentSpoofing(true)}
          >
            {t("identity.on")}
          </Button>
          <Button
            id="chuzz-user-agent-spoof-off"
            variant={userAgent.spoofing ? "outline" : "solid"}
            size="sm"
            aria-label={`${t("identity.label")} ${t("identity.off")}`}
            aria-pressed={userAgent.spoofing ? "false" : "true"}
            state={userAgent.locked ? "disabled" : undefined}
            onClick={() => void setUserAgentSpoofing(false)}
          >
            {t("identity.off")}
          </Button>
        </Flex>
      </Flex>
    </Flex>
  );
}
