import type { AppSettings } from "./types";

export type UiPreferences = Pick<AppSettings, "locale" | "theme" | "textSize">;

export const defaultUiPreferences: UiPreferences = {
  locale: "system",
  theme: "system",
  textSize: "large",
};

const storageKey = "cleanerx.ui-preferences.v1";

export function readCachedPreferences(): UiPreferences | undefined {
  try {
    const stored = window.localStorage.getItem(storageKey);
    if (!stored) return undefined;
    const value = JSON.parse(stored) as Partial<UiPreferences>;
    if (!isLocale(value.locale) || !isTheme(value.theme) || !isTextSize(value.textSize)) return undefined;
    return { locale: value.locale, theme: value.theme, textSize: value.textSize };
  } catch {
    return undefined;
  }
}

export function cachePreferences(preferences: UiPreferences) {
  try {
    const { locale, theme, textSize } = preferences;
    window.localStorage.setItem(storageKey, JSON.stringify({ locale, theme, textSize }));
  } catch {
    // The native settings file remains authoritative when WebView storage is unavailable.
  }
}

export function resolveLocalePreference(
  preference: AppSettings["locale"],
  languages: readonly string[] = navigator.languages,
): "zh" | "en" {
  if (preference !== "system") return preference;
  const language = languages[0] ?? navigator.language;
  return language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function resolveThemePreference(
  preference: AppSettings["theme"],
  prefersDark = systemPrefersDark(),
): "light" | "dark" {
  if (preference !== "system") return preference;
  return prefersDark ? "dark" : "light";
}

function systemPrefersDark() {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function isLocale(value: unknown): value is AppSettings["locale"] {
  return value === "system" || value === "zh" || value === "en";
}

function isTheme(value: unknown): value is AppSettings["theme"] {
  return value === "system" || value === "light" || value === "dark";
}

function isTextSize(value: unknown): value is AppSettings["textSize"] {
  return value === "standard" || value === "large" || value === "extraLarge";
}
