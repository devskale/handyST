import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../../ui";
import { useSettings } from "../../../hooks/useSettings";

interface RecordingModeSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Recording mode: how the hotkey drives capture.
 *   hold  = push-to-talk  (hold while speaking, release to stop)
 *   click = toggle        (tap to start, tap again to stop)
 * Maps onto the persisted push_to_talk boolean.
 */
export const RecordingModeSelector: React.FC<RecordingModeSelectorProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const holdMode = getSetting("push_to_talk") ?? true;
    const disabled = isUpdating("push_to_talk");

    const modes = [
      {
        id: "hold" as const,
        active: holdMode,
        label: t("settings.general.recordingMode.hold.label"),
        title: t("settings.general.recordingMode.hold.description"),
        onClick: () => updateSetting("push_to_talk", true),
      },
      {
        id: "click" as const,
        active: !holdMode,
        label: t("settings.general.recordingMode.click.label"),
        title: t("settings.general.recordingMode.click.description"),
        onClick: () => updateSetting("push_to_talk", false),
      },
    ];

    return (
      <SettingContainer
        title={t("settings.general.recordingMode.label")}
        description={t("settings.general.recordingMode.description")}
        descriptionMode={descriptionMode}
        layout="horizontal"
        grouped={grouped}
      >
        <div className="flex gap-1 p-1 rounded-lg border border-mid-gray/20 bg-mid-gray/5">
          {modes.map((mode) => (
            <button
              key={mode.id}
              type="button"
              title={mode.title}
              disabled={disabled}
              onClick={mode.onClick}
              className={`px-3 py-1.5 text-xs font-medium rounded-md transition-colors ${
                mode.active
                  ? "bg-background border border-logo-primary/40 text-text shadow-sm"
                  : "border border-transparent text-mid-gray hover:text-text hover:bg-background-ui/40"
              } ${disabled ? "opacity-50 cursor-default" : "cursor-pointer"}`}
            >
              {mode.label}
            </button>
          ))}
        </div>
      </SettingContainer>
    );
  });

RecordingModeSelector.displayName = "RecordingModeSelector";
