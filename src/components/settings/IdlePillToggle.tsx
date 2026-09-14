import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { commands } from "@/bindings";

/** sttts: persistent dictation pill while idle — click to start without keys. */
export const IdlePillToggle: React.FC<{
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}> = ({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  return (
    <ToggleSwitch
      checked={getSetting("overlay_idle_pill") || false}
      onChange={(enabled: boolean) => {
        updateSetting("overlay_idle_pill", enabled);
        // Immediate visual feedback for both directions
        if (enabled) commands.showIdlePillIfEnabled();
        else commands.hideOverlay();
      }}
      isUpdating={isUpdating("overlay_idle_pill")}
      label={t("settings.general.idlePill.label")}
      description={t("settings.general.idlePill.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    />
  );
};
