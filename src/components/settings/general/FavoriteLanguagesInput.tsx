import React from "react";
import { useTranslation } from "react-i18next";
import { SettingContainer } from "../../ui";
import { Input } from "../../ui/Input";
import { useSettings } from "../../../hooks/useSettings";

/**
 * Which languages appear as chips in the recording overlay's quick switcher.
 * Comma-separated codes ("auto" allowed); saved on blur.
 */
export const FavoriteLanguagesInput: React.FC<{
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}> = ({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const favorites = (getSetting("favorite_languages") ?? ["auto", "de", "en"]).join(", ");
  const [draft, setDraft] = React.useState(favorites);
  React.useEffect(() => setDraft(favorites), [favorites]);

  const save = () => {
    const parsed = draft
      .split(",")
      .map((code) => code.trim())
      .filter((code) => code.length > 0);
    if (parsed.length === 0) return;
    updateSetting("favorite_languages", parsed);
  };

  return (
    <SettingContainer
      title={t("settings.general.favoriteLanguages.label")}
      description={t("settings.general.favoriteLanguages.description")}
      descriptionMode={descriptionMode}
      layout="horizontal"
      grouped={grouped}
    >
      <Input
        type="text"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={save}
        placeholder="auto, de, en"
        variant="compact"
        disabled={isUpdating("favorite_languages")}
        className="flex-1 min-w-[200px]"
      />
    </SettingContainer>
  );
};
