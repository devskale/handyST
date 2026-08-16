import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../../ui";
import { Select } from "../../ui/Select";
import { useSettings } from "../../../hooks/useSettings";

interface DictationShortcutSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * macOS-dictation-style trigger for starting/stopping dictation without a
 * key chord: the mic key, or double-tapping Control / Globe / a Command side.
 * Independent of (and in addition to) the regular transcribe hotkey; always
 * toggles, regardless of the recording mode.
 */
export const DictationShortcutSelector: React.FC<
  DictationShortcutSelectorProps
> = React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const value = getSetting("dictation_shortcut") || "none";

  const options = [
    { value: "none", label: t("settings.general.dictationShortcut.options.none") },
    { value: "mic_key", label: t("settings.general.dictationShortcut.options.micKey") },
    { value: "ctrl_double", label: t("settings.general.dictationShortcut.options.ctrlDouble") },
    { value: "globe_double", label: t("settings.general.dictationShortcut.options.globeDouble") },
    {
      value: "cmd_left_double",
      label: t("settings.general.dictationShortcut.options.cmdLeftDouble"),
    },
    {
      value: "cmd_right_double",
      label: t("settings.general.dictationShortcut.options.cmdRightDouble"),
    },
  ];

  return (
    <SettingContainer
      title={t("settings.general.dictationShortcut.label")}
      description={t("settings.general.dictationShortcut.description")}
      descriptionMode={descriptionMode}
      layout="horizontal"
      grouped={grouped}
    >
      <Select
        value={value}
        options={options}
        disabled={isUpdating("dictation_shortcut")}
        onChange={(selected) =>
          updateSetting("dictation_shortcut", selected ?? "none")
        }
        className="min-w-[220px]"
      />
    </SettingContainer>
  );
});

DictationShortcutSelector.displayName = "DictationShortcutSelector";
