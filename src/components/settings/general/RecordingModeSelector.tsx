import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../../ui";
import { useSettings } from "../../../hooks/useSettings";

interface ShortcutBehaviorSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * How the dictation key drives recording — upstream's ShortcutActivation:
 *   auto  = hold-or-toggle (hold to record; a quick tap locks recording on)
 *   hold  = classic push-to-talk
 *   toggle= press to start, press again to stop
 */
export const ShortcutBehaviorSelector: React.FC<
  ShortcutBehaviorSelectorProps
> = React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const mode = getSetting("shortcut_activation") || "hold_or_toggle";
  const disabled = isUpdating("shortcut_activation");

  const modes = [
    {
      id: "hold_or_toggle" as const,
      active: mode === "hold_or_toggle",
      label: t("settings.general.shortcutBehavior.options.auto"),
      title: t("settings.general.shortcutBehavior.descriptions.hold_or_toggle"),
      onClick: () => updateSetting("shortcut_activation", "hold_or_toggle"),
    },
    {
      id: "push_to_talk" as const,
      active: mode === "push_to_talk",
      label: t("settings.general.shortcutBehavior.options.hold"),
      title: t("settings.general.shortcutBehavior.descriptions.push_to_talk"),
      onClick: () => updateSetting("shortcut_activation", "push_to_talk"),
    },
    {
      id: "toggle" as const,
      active: mode === "toggle",
      label: t("settings.general.shortcutBehavior.options.toggle"),
      title: t("settings.general.shortcutBehavior.descriptions.toggle"),
      onClick: () => updateSetting("shortcut_activation", "toggle"),
    },
  ];

  return (
    <SettingContainer
      title={t("settings.general.shortcutBehavior.title")}
      description={t("settings.general.shortcutBehavior.description")}
      descriptionMode={descriptionMode}
      layout="horizontal"
      grouped={grouped}
    >
      <div className="flex gap-1 p-1 rounded-lg border border-mid-gray/20 bg-mid-gray/5">
        {modes.map((m) => (
          <button
            key={m.id}
            type="button"
            title={m.title}
            disabled={disabled}
            onClick={m.onClick}
            className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
              m.active
                ? "bg-background border border-logo-primary/40 text-text shadow-sm"
                : "border border-transparent text-mid-gray hover:text-text hover:bg-background-ui/40"
            } ${disabled ? "opacity-50 cursor-default" : "cursor-pointer"}`}
          >
            {m.label}
          </button>
        ))}
      </div>
    </SettingContainer>
  );
});

ShortcutBehaviorSelector.displayName = "ShortcutBehaviorSelector";
