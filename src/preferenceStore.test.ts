import { describe, expect, it } from "vitest";
import {
  cachePreferences,
  readCachedPreferences,
  resolveLocalePreference,
  resolveThemePreference,
} from "./preferenceStore";

describe("UI preferences", () => {
  it("resolves system language and appearance", () => {
    expect(resolveLocalePreference("system", ["zh-Hans-CN"])).toBe("zh");
    expect(resolveLocalePreference("system", ["fr-FR"])).toBe("en");
    expect(resolveThemePreference("system", true)).toBe("dark");
    expect(resolveThemePreference("system", false)).toBe("light");
  });

  it("caches only recognized preference values", () => {
    cachePreferences({ locale: "zh", theme: "dark" });
    expect(readCachedPreferences()).toEqual({ locale: "zh", theme: "dark" });

    window.localStorage.setItem("cleanerx.ui-preferences.v1", JSON.stringify({ locale: "xx", theme: "neon" }));
    expect(readCachedPreferences()).toBeUndefined();
  });
});
