import { useEffect, useState } from "react";

import { invoke } from "@/lib/invoke";
import type { PaletteConfig } from "@/lib/labbyClient";

/** Loads palette settings, applies theme changes, and persists edits. */
export function usePaletteConfig() {
  const [config, setConfig] = useState<PaletteConfig | null>(null);
  const [draftConfig, setDraftConfig] = useState<PaletteConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);

  useEffect(() => {
    invoke<PaletteConfig>("load_palette_config")
      .then((nextConfig) => {
        setConfig(nextConfig);
        setDraftConfig(nextConfig);
      })
      .catch((err) => {
        setConfigError(String(err));
        void invoke<PaletteConfig>("load_palette_default_config")
          .then((fallbackConfig) => {
            setConfig(fallbackConfig);
            setDraftConfig(fallbackConfig);
          })
          .catch(() => {
            setConfig(null);
            setDraftConfig(null);
          });
      });
  }, []);

  useEffect(() => {
    if (!config) return;
    const root = document.documentElement;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const applyTheme = () => {
      const useLight = config.theme === "light" || (config.theme === "system" && media.matches);
      root.classList.toggle("light", useLight);
      root.classList.toggle("dark", !useLight);
    };
    applyTheme();
    media.addEventListener("change", applyTheme);
    return () => media.removeEventListener("change", applyTheme);
  }, [config]);

  /** Persists `draftConfig`. Rethrows on failure so callers (e.g. the
   * Settings panel's save/test-connection buttons) can react to it. */
  async function saveSettings() {
    if (!draftConfig) return;
    try {
      const nextConfig = await invoke<PaletteConfig>("save_palette_settings", {
        settings: draftConfig,
      });
      setConfig(nextConfig);
      setDraftConfig(nextConfig);
      setConfigError(null);
    } catch (err) {
      const message = String(err);
      setConfigError(message);
      throw err instanceof Error ? err : new Error(message);
    }
  }

  return { config, draftConfig, setDraftConfig, configError, saveSettings };
}
