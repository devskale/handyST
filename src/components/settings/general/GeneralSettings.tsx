import React from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { MicrophoneSelector } from "../MicrophoneSelector";
import { ChannelSelector } from "../ChannelSelector";
import { ShortcutInput } from "../ShortcutInput";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { OutputDeviceSelector } from "../OutputDeviceSelector";
import { ShortcutBehaviorSelector } from "./RecordingModeSelector";
import { DictationShortcutSelector } from "./DictationShortcutSelector";
import { LanguageSelector } from "../LanguageSelector";
import { FavoriteLanguagesInput } from "./FavoriteLanguagesInput";
import { AudioFeedback } from "../AudioFeedback";
import { useSettings } from "../../../hooks/useSettings";
import { VolumeSlider } from "../VolumeSlider";
import { MuteWhileRecording } from "../MuteWhileRecording";

export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled, getSetting } = useSettings();
  const pushToTalk =
    (getSetting("shortcut_activation") || "hold_or_toggle") === "push_to_talk";
  const isLinux = type() === "linux";
  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.general.title")}>
        <ShortcutInput shortcutId="transcribe" grouped={true} />
        <ShortcutBehaviorSelector descriptionMode="tooltip" grouped={true} />
        <DictationShortcutSelector descriptionMode="tooltip" grouped={true} />
        <LanguageSelector descriptionMode="tooltip" grouped={true} />
        <FavoriteLanguagesInput descriptionMode="tooltip" grouped={true} />
        {/* Cancel shortcut is hidden with push-to-talk (release key cancels) and on Linux (dynamic shortcut instability) */}
        {!isLinux && !pushToTalk && (
          <ShortcutInput shortcutId="cancel" grouped={true} />
        )}
      </SettingsGroup>
      <SettingsGroup title={t("settings.sound.title")}>
        <MicrophoneSelector descriptionMode="tooltip" grouped={true} />
        <ChannelSelector descriptionMode="tooltip" grouped={true} />
        <MuteWhileRecording descriptionMode="tooltip" grouped={true} />
        <AudioFeedback descriptionMode="tooltip" grouped={true} />
        <OutputDeviceSelector
          descriptionMode="tooltip"
          grouped={true}
          disabled={!audioFeedbackEnabled}
        />
        <VolumeSlider disabled={!audioFeedbackEnabled} />
      </SettingsGroup>
    </div>
  );
};
