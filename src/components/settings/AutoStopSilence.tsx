import React from "react";
import { useTranslation } from "react-i18next";
import { Slider } from "../ui/Slider";
import { useSettings } from "../../hooks/useSettings";

/**
 * Silence auto-stop: end the recording automatically after this much silence
 * following speech (like macOS dictation). 0 = off. Requires Voice Activity
 * Detection. Applies from the next recording on.
 */
export const AutoStopSilence: React.FC<{
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}> = ({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { settings, updateSetting, resetSetting, isUpdating } = useSettings();

  return (
    <Slider
      value={settings?.auto_stop_silence_ms ?? 0}
      onChange={(value: number) => updateSetting("auto_stop_silence_ms", value)}
      onReset={() => resetSetting("auto_stop_silence_ms")}
      isResetting={isUpdating("auto_stop_silence_ms")}
      min={0}
      max={5000}
      step={250}
      label={t("settings.advanced.autoStopSilence.title")}
      description={t("settings.advanced.autoStopSilence.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      formatValue={(v: number) => (v === 0 ? "off" : `${(v / 1000).toFixed(2)}s`)}
    />
  );
};
