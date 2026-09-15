import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App, SettingsWindow } from "./App";
import { activeLocale, t } from "./i18n";
import "./index.css";

const container = document.getElementById("root");
if (!container) throw new Error("missing #root");

// Two things the markup cannot state for itself. The platform decides what
// each OS does its own way — body size, colours, where Settings lives — so
// the stylesheet and the toolbar can ask it. The language is whichever one
// the catalog is answering in, so a screen reader pronounces the Chinese
// build as Chinese.
const agent = navigator.userAgent;
document.documentElement.dataset.platform = agent.includes("Mac")
  ? "darwin"
  : agent.includes("Windows")
    ? "win32"
    : "linux";
document.documentElement.lang = activeLocale();

// Whether this is the window being typed into, which a Mac shows by dimming
// the frame of every other one.
const markActive = () => {
  document.documentElement.dataset.active = String(document.hasFocus());
};
window.addEventListener("focus", markActive);
window.addEventListener("blur", markActive);
markActive();

// Return presses the filled button, as it does in a dialog on either
// platform — unless focus is somewhere with its own use for Return: a field
// submits its form, a focused button presses itself. Each view draws at
// most one filled button, so there is no choosing between them.
window.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.repeat || event.defaultPrevented) return;
  const focus = event.target as Element;
  if (focus.closest("input, textarea, select, button, a[href], [contenteditable]")) return;
  document.querySelector<HTMLButtonElement>(".push-default:not(:disabled)")?.click();
});

// The Mac's Settings window loads this same page, named by its hash. Its
// title is set before the first paint, which is when the window is shown.
const settings = location.hash === "#settings";
if (settings) document.title = t("settings.windowTitle");

createRoot(container).render(
  <StrictMode>{settings ? <SettingsWindow /> : <App />}</StrictMode>,
);
