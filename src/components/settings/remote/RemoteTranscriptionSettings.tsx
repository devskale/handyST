import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import type { RemoteCheckResult } from "@/bindings";
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
  const [check, setCheck] = useState<RemoteCheckResult | null>(null);
  const [checking, setChecking] = useState(false);

  const runCheck = () => {
    setChecking(true);
    commands
      .checkRemoteTranscription()
      .then((result) => {
        if (result.status === "ok") setCheck(result.data);
        else
          setCheck({
            ok: false,
            components: [
              { name: "gateway", ok: false, detail: String(result.error) },
            ],
          });
      })
      .catch((e) =>
        setCheck({
          ok: false,
          components: [{ name: "gateway", ok: false, detail: String(e) }],
        }),
      )
      .finally(() => setChecking(false));
  };

  useEffect(() => {
    runCheck();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- run once on mount
  }, []);

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
            placeholder="http://localhost:3001"
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
            onBlur={(event) => saveApiKey(event.target.value)}
            placeholder="sk-..."
            variant="compact"
            className="flex-1 min-w-[240px]"
          />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title={t("settings.remoteTranscription.check.title")}>
        <SettingContainer
          title={t("settings.remoteTranscription.check.title")}
          description={t("settings.remoteTranscription.check.description")}
        >
          <div className="flex flex-col gap-2 w-full">
            {check?.components.map((c) => (
              <div
                key={c.name}
                className="flex items-center justify-between gap-3 text-sm"
              >
                <span className="flex items-center gap-2">
                  <span
                    className={
                      c.ok
                        ? "text-green-600 dark:text-green-400"
                        : "text-red-600 dark:text-red-400"
                    }
                  >
                    {c.ok ? "✔" : "✘"}
                  </span>
                  {t(
                    `settings.remoteTranscription.check.components.${c.name}`,
                  )}
                </span>
                <span className="text-right text-xs text-neutral-500 dark:text-neutral-400 truncate max-w-[55%]">
                  {c.detail}
                </span>
              </div>
            ))}
            <button
              onClick={runCheck}
              disabled={checking}
              className="mt-1 self-start px-3 py-1.5 rounded-md text-sm bg-neutral-200 hover:bg-neutral-300 dark:bg-neutral-700 dark:hover:bg-neutral-600 disabled:opacity-50"
            >
              {checking
                ? t("settings.remoteTranscription.check.running")
                : t("settings.remoteTranscription.check.button")}
            </button>
          </div>
        </SettingContainer>
      </SettingsGroup>
    </div>
  );
};
