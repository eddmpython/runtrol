import type { ThemeMode } from "./domain";

const THEME_KEY = "runtrol.theme";

export function initialTheme(): ThemeMode {
  try {
    const stored = window.localStorage.getItem(THEME_KEY);
    if (stored === "light" || stored === "dark") {
      return stored;
    }
  } catch (error) {
    // The theme is optional preference state. A blocked store falls back to the operating system.
    console.warn("cannot read the saved theme", error);
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function applyTheme(mode: ThemeMode): void {
  document.documentElement.dataset.theme = mode;
  document.documentElement.dataset.astryxMedia = mode;
  try {
    window.localStorage.setItem(THEME_KEY, mode);
  } catch (error) {
    // Rendering the chosen mode matters more than persisting an optional preference.
    console.warn("cannot save the selected theme", error);
  }
}
