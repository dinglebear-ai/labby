// Single invoke wrapper used by every caller (App, labbyClient, oauthClient).
//
// In the Tauri runtime it forwards to the real `@tauri-apps/api/core` invoke.
// In a plain browser (vite dev — used for design iteration/screenshots) it
// returns benign stubs so the UI stays renderable without a Tauri backend.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export const isTauriRuntime =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Shared Tauri window handle with a browser fallback. In the Tauri runtime it is
// the real window (event listeners wired); under `vite dev` it is a no-op stub so
// `appWindow.listen(...)` is always callable. Consumed by the palette lifecycle
// and OAuth session hooks.
export const appWindow = isTauriRuntime
  ? getCurrentWindow()
  : {
      listen: async () => () => undefined,
    };

export async function invoke<T = unknown>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isTauriRuntime) return tauriInvoke<T>(command, args);
  switch (command) {
    case "fetch_catalog":
      return { ok: true, status: 200, payload: { services: [] } } as T;
    case "fetch_launcher_catalog":
      return { ok: true, status: 200, payload: { fingerprint: "browser-fallback", entries: [] } } as T;
    case "dispatch_action":
      return { ok: true, status: 200, payload: null } as T;
    case "execute_launcher_entry":
      return {
        ok: false,
        status: 501,
        payload: {
          kind: "unsupported_surface",
          message: "Launcher execution is only available in the desktop app",
        },
      } as T;
    case "load_palette_config":
    case "load_palette_default_config":
      return {
        serverUrl: "http://localhost:8765",
        staticToken: null,
        shortcut: "Ctrl+Shift+Space",
        theme: "dark",
        hideOnBlur: false,
        openResultsInline: true,
        showFooterHints: false,
      } as T;
    case "save_palette_settings":
      return (args?.settings ?? args) as T;
    case "hide_palette":
    case "show_palette":
    case "resize_palette":
    case "set_blur_dismiss":
    case "toggle_maximize":
      return undefined as T;
    case "labby_oauth_status":
    case "labby_oauth_logout":
      return { signedIn: false, scope: null, expiresAtUnix: null, serverUrl: null } as T;
    case "labby_oauth_login":
      throw new Error("OAuth login is only available in the desktop app");
    default:
      throw new Error(`${command} is only available in the Tauri runtime`);
  }
}
