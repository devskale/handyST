import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { SettingContainer, SettingsGroup } from "../../ui";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { Input } from "../../ui/Input";

/**
 * sttts: configuration for the remote (OpenAI-compatible) transcription
 * endpoint that replaces all local ML inference. The API key lives in the OS
 * keychain (not in settings.json), so it is fetched/stored via dedicated
 * commands rather than the settings store.
 */
export const RemoteTranscriptionSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();

  const enabled = getSetting("remote_transcription_enabled") || false;
  const baseUrl = getSetting("remote_transcription_base_url") || "";
  const model = getSetting("remote_transcription_model") || "";

  const [apiKey, setApiKey] = useState("");

  useEffect(() => {
    commands
      .getRemoteTranscriptionApiKey()
      .then((result) => {
        if (result.status === "ok") setApiKey(result.data);
      })
      .catch((e) => console.warn("Failed to read API key:", e));
  }, []);

  const saveApiKey = (value: string) => {
    setApiKey(value);
    commands
      .changeRemoteTranscriptionApiKeySetting(value)
      .then((result) => {
        if (result.status === "error") console.warn(result.error);
      })
      .catch((e) => console.warn("Failed to store API key:", e));
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.remoteTranscription.title")}>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) =>
            updateSetting("remote_transcription_enabled", value)
          }
          isUpdating={isUpdating("remote_transcription_enabled")}
          label={t("settings.remoteTranscription.enabled.label")}
          description={t("settings.remoteTranscription.enabled.description")}
          descriptionMode="tooltip"
          grouped={true}
        />
        <SettingContainer
          title={t("settings.remoteTranscription.baseUrl.label")}
          description={t("settings.remoteTranscription.baseUrl.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Input
            type="text"
            value={baseUrl}
            onChange={(event) =>
              updateSetting(
                "remote_transcription_base_url",
                event.target.value,
              )
            }
            placeholder="http://dgxp:3001"
            variant="compact"
            className="flex-1 min-w-[240px]"
          />
        </SettingContainer>
        <SettingContainer
          title={t("settings.remoteTranscription.model.label")}
          description={t("settings.remoteTranscription.model.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Input
            type="text"
            value={model}
            onChange={(event) =>
              updateSetting("remote_transcription_model", event.target.value)
            }
            placeholder="nemotron-3.5-asr-streaming-0.6b"
            variant="compact"
            className="flex-1 min-w-[240px]"
          />
        </SettingContainer>
        <SettingContainer
          title={t("settings.remoteTranscription.apiKey.label")}
          description={t("settings.remoteTranscription.apiKey.description")}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Input
            type="password"
            value={apiKey}
            onChange={(event) => saveApiKey(event.target.value)}
            placeholder="sk-..."
            variant="compact"
            className="flex-1 min-w-[240px]"
          />
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
