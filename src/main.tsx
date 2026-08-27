import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./i18n";
import "./styles.css";
import App from "./App";
import { applyPreferences } from "./preferences";
import { defaultUiPreferences, readCachedPreferences } from "./preferenceStore";

void applyPreferences(readCachedPreferences() ?? defaultUiPreferences);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
