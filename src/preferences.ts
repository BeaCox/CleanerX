import i18n from "./i18n";
import { resolveLocalePreference, resolveThemePreference, type UiPreferences } from "./preferenceStore";

export async function applyPreferences(preferences: UiPreferences) {
  const language = resolveLocalePreference(preferences.locale);
  const theme = resolveThemePreference(preferences.theme);
  const root = document.documentElement;

  root.lang = language === "zh" ? "zh-CN" : "en";
  root.dataset.theme = theme;
  root.dataset.textSize = preferences.textSize;
  root.style.colorScheme = theme;

  if (i18n.resolvedLanguage !== language) await i18n.changeLanguage(language);
}

export function watchSystemPreferences(preferences: UiPreferences) {
  const media = typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : undefined;
  const handleLanguage = () => {
    if (preferences.locale === "system") void applyPreferences(preferences);
  };
  const handleTheme = () => {
    if (preferences.theme === "system") void applyPreferences(preferences);
  };

  window.addEventListener("languagechange", handleLanguage);
  media?.addEventListener("change", handleTheme);
  return () => {
    window.removeEventListener("languagechange", handleLanguage);
    media?.removeEventListener("change", handleTheme);
  };
}
