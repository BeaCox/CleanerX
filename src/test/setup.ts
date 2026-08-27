import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach } from "vitest";
import { cleanup } from "@testing-library/react";
import i18n from "../i18n";

beforeEach(async () => {
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("style");
  document.documentElement.lang = "";
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});
