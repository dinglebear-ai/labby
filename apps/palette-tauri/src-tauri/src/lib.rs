use std::{
    fmt::Display,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Size,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

mod labby_bridge;
mod oauth;
mod persistence;
mod window_events;

use labby_bridge::BridgeClient;
use persistence::*;

/// Log a warning through `tracing`. Replaces the former Axon `diag` wrapper; see
/// `docs/dev/OBSERVABILITY.md` — use `tracing`, never a custom logger.
pub(crate) fn warn(message: impl Display) {
    tracing::warn!("{message}");
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LabbySettings {
    server_url: String,
    static_token: Option<String>,
    shortcut: String,
    theme: PaletteTheme,
    hide_on_blur: bool,
    open_results_inline: bool,
    show_footer_hints: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum PaletteTheme {
    System,
    Dark,
    Light,
}

const DEFAULT_SERVER_URL: &str = "http://localhost:8765";
const DEFAULT_SHORTCUT: &str = "Ctrl+Shift+Space";
const SETTINGS_FILE: &str = "settings.json";

// Runtime gate for hide-on-blur, toggled by the frontend. The launcher hides on
// blur (click-away dismiss), but while a result/settings view is open we keep it
// up so resizing or copying from another window doesn't make it vanish.
// Checked together with the `hide_on_blur` user preference in the
// `WindowEvent::Focused(false)` handler.
struct BlurDismiss(AtomicBool);

/// Tracks the shortcut label currently registered so we can unregister only
/// that specific shortcut (rather than calling `unregister_all`) when the user
/// changes the keybinding.
struct ActiveShortcut(Mutex<Option<String>>);

fn log_palette_warning(context: &str, err: impl Display) {
    warn(format!("{context}: {err}"));
}

#[tauri::command]
fn load_palette_config(app: AppHandle) -> Result<LabbySettings, String> {
    merged_settings(&app)
}

#[tauri::command]
fn load_palette_default_config() -> LabbySettings {
    default_settings()
}

#[tauri::command]
fn save_palette_settings(app: AppHandle, settings: LabbySettings) -> Result<LabbySettings, String> {
    let settings = normalize_settings(settings);
    // 1. Persist palette-only preferences.
    save_palette_prefs(&app, &settings)?;
    // 2. Only mutate runtime state (shortcut) after the write succeeds.
    update_shortcut(&app, &settings)?;
    Ok(settings)
}

fn save_palette_prefs(app: &AppHandle, settings: &LabbySettings) -> Result<(), String> {
    write_settings(app, settings).map_err(|err| err.to_string())
}

fn update_shortcut(app: &AppHandle, settings: &LabbySettings) -> Result<(), String> {
    register_configured_shortcut(app, settings)
}

#[tauri::command]
fn hide_palette(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?
        .hide()
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn show_palette(app: AppHandle) -> Result<(), String> {
    show_main_window(&app)
}

#[tauri::command]
fn resize_palette(app: AppHandle, width: f64, height: f64, shadow: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    // A maximized window ignores set_size on Windows; drop maximize first so the
    // auto-sizer (and the next launcher open) always lands at the intended size.
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    window
        .set_size(Size::Logical(LogicalSize { width, height }))
        .map_err(|err| err.to_string())?;
    // Per-view native shadow toggle (see useWindowChrome.ts for the policy).
    let _ = window.set_shadow(shadow);
    window.center().map_err(|err| err.to_string())
}

#[tauri::command]
fn toggle_maximize(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    if window.is_maximized().map_err(|err| err.to_string())? {
        window.unmaximize().map_err(|err| err.to_string())
    } else {
        window.maximize().map_err(|err| err.to_string())
    }
}

#[tauri::command]
fn set_blur_dismiss(state: tauri::State<'_, BlurDismiss>, enabled: bool) {
    state.0.store(enabled, Ordering::Relaxed);
}

fn merged_settings(app: &AppHandle) -> Result<LabbySettings, String> {
    let persisted = read_settings_result(app)?;
    let defaults = default_settings();
    Ok(merge_settings(persisted, defaults))
}

fn merged_settings_or_default(app: &AppHandle) -> LabbySettings {
    match merged_settings(app) {
        Ok(settings) => settings,
        Err(err) => {
            warn(&err);
            default_settings()
        }
    }
}

fn merge_settings(persisted: PartialPaletteSettings, defaults: LabbySettings) -> LabbySettings {
    normalize_settings(LabbySettings {
        server_url: persisted.server_url.unwrap_or(defaults.server_url),
        static_token: persisted.static_token.unwrap_or(defaults.static_token),
        shortcut: persisted
            .shortcut
            .unwrap_or_else(|| DEFAULT_SHORTCUT.to_string()),
        theme: persisted.theme.unwrap_or(PaletteTheme::System),
        hide_on_blur: persisted.hide_on_blur.unwrap_or(true),
        open_results_inline: persisted.open_results_inline.unwrap_or(true),
        show_footer_hints: persisted.show_footer_hints.unwrap_or(false),
    })
}

fn default_settings() -> LabbySettings {
    let server_url = default_server_url(
        value_for("LABBY_API_URL").as_deref(),
        value_for("LABBY_PUBLIC_URL").as_deref(),
    );
    let static_token = value_for("LABBY_MCP_HTTP_TOKEN")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    LabbySettings {
        server_url,
        static_token,
        shortcut: DEFAULT_SHORTCUT.to_string(),
        theme: PaletteTheme::System,
        hide_on_blur: true,
        open_results_inline: true,
        show_footer_hints: false,
    }
}

fn default_server_url(api_url: Option<&str>, public_url: Option<&str>) -> String {
    api_url
        .or(public_url)
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartialPaletteSettings {
    server_url: Option<String>,
    static_token: Option<Option<String>>,
    shortcut: Option<String>,
    theme: Option<PaletteTheme>,
    hide_on_blur: Option<bool>,
    open_results_inline: Option<bool>,
    show_footer_hints: Option<bool>,
}

fn normalize_settings(mut settings: LabbySettings) -> LabbySettings {
    settings.server_url = normalize_server_url(&settings.server_url);
    if settings.server_url.is_empty() {
        settings.server_url = DEFAULT_SERVER_URL.to_string();
    }
    settings.static_token = settings
        .static_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());
    settings.shortcut = normalize_shortcut_label(&settings.shortcut);
    settings
}

/// Normalise a user-entered server URL down to its origin (scheme + host +
/// port), silently dropping any path/query/fragment.
///
/// Labby exposes multiple surfaces at the same host — `/mcp` (MCP transport),
/// `/v1/*` (this app's REST API), `/authorize` (OAuth) — so it's an easy
/// mistake to paste the MCP URL (e.g. `https://labby.example.com/mcp`) into
/// this field. Silently stripping the path rather than hard-erroring means
/// that mistake just works instead of breaking OAuth status/login with an
/// opaque "invalid server URL" failure.
fn normalize_server_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.starts_with("localhost") || trimmed.starts_with("127.0.0.1") {
        format!("http://{trimmed}")
    } else {
        format!("https://{trimmed}")
    };
    match reqwest::Url::parse(&with_scheme) {
        Ok(url) if url.host_str().is_some() => url.origin().ascii_serialization(),
        _ => with_scheme,
    }
}

fn normalize_shortcut_label(shortcut: &str) -> String {
    match shortcut.trim().to_ascii_lowercase().as_str() {
        "alt+space" | "option+space" => "Alt+Space".to_string(),
        "ctrl+space" | "control+space" => "Ctrl+Space".to_string(),
        "cmd+shift+space" | "command+shift+space" | "super+shift+space" => {
            "Cmd+Shift+Space".to_string()
        }
        _ => DEFAULT_SHORTCUT.to_string(),
    }
}

/// Validate a saved Labby server URL. `normalize_server_url` already strips
/// any path/query/fragment down to the origin, so this only needs to reject
/// an empty/unparsable value or a non-http(s) scheme.
pub(crate) fn validate_saved_server_url(server_url: &str) -> Result<String, String> {
    let server_url = normalize_server_url(server_url);
    if server_url.is_empty() {
        return Err("no Labby server URL is configured — set one in Settings".to_string());
    }
    let parsed = reqwest::Url::parse(&server_url)
        .map_err(|err| format!("saved Labby server URL is invalid: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("saved Labby server URL must use http or https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("saved Labby server URL must include a host".to_string());
    }
    Ok(server_url)
}

fn shortcut_for_label(label: &str) -> Shortcut {
    match normalize_shortcut_label(label).as_str() {
        "Alt+Space" => Shortcut::new(Some(Modifiers::ALT), Code::Space),
        "Ctrl+Space" => Shortcut::new(Some(Modifiers::CONTROL), Code::Space),
        "Cmd+Shift+Space" => Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space),
        _ => Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space),
    }
}

fn register_configured_shortcut(app: &AppHandle, settings: &LabbySettings) -> Result<(), String> {
    let new_label = normalize_shortcut_label(&settings.shortcut);
    let new_shortcut = shortcut_for_label(&new_label);

    // Unregister only the previously registered shortcut if we know what it is,
    // rather than calling `unregister_all` which would also unregister shortcuts
    // registered by other parts of the app.
    if let Ok(mut guard) = app.state::<ActiveShortcut>().0.lock() {
        // Already registered with this exact label (e.g. Settings saved again
        // with the shortcut unchanged) — re-registering an already-registered
        // hotkey errors ("HotKey already registered"), so short-circuit.
        if guard.as_deref() == Some(new_label.as_str()) {
            return Ok(());
        }
        if let Some(old_label) = guard.take() {
            let old_shortcut = shortcut_for_label(&old_label);
            if let Err(err) = app.global_shortcut().unregister(old_shortcut) {
                warn(format!(
                    "failed to unregister old shortcut '{old_label}': {err}"
                ));
            }
        }
        app.global_shortcut()
            .register(new_shortcut)
            .map_err(|err| err.to_string())?;
        *guard = Some(new_label);
    } else {
        // Mutex poisoned — fall back to unregister_all for safety.
        app.global_shortcut()
            .unregister_all()
            .map_err(|err| err.to_string())?;
        app.global_shortcut()
            .register(new_shortcut)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    window
        .set_size(Size::Logical(LogicalSize {
            // Compact launcher — matches COMPACT in useWindowChrome.ts (bar + inset).
            width: 720.0,
            height: 92.0,
        }))
        .map_err(|err| err.to_string())?;
    // Compact floats a CSS-glowing bar; keep the native shadow off (JS re-asserts).
    let _ = window.set_shadow(false);
    window.center().map_err(|err| err.to_string())?;
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())?;
    if let Err(err) = window.emit("palette://shown", ()) {
        log_palette_warning("failed to emit shown event", err);
    }
    Ok(())
}

fn toggle_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            if let Err(err) = window.hide() {
                log_palette_warning("failed to hide main window", err);
            }
        }
        _ => {
            if let Err(err) = show_main_window(app) {
                log_palette_warning("failed to show main window", err);
            }
        }
    }
}

fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Palette", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Labby Palette", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &settings, &quit])?;

    let icon = app.default_window_icon().cloned();
    let mut tray = TrayIconBuilder::new()
        .tooltip("Labby Palette")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Err(err) = show_main_window(app) {
                    log_palette_warning("failed to show main window from tray", err);
                }
            }
            "settings" => {
                if let Err(err) = show_main_window(app) {
                    log_palette_warning("failed to show main window for settings", err);
                }
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(err) = window.emit("palette://open-settings", ()) {
                        log_palette_warning("failed to emit open settings event", err);
                    }
                } else {
                    log_palette_warning("failed to open settings", "main window not found");
                }
            }
            "quit" => {
                if let Err(err) = app.global_shortcut().unregister_all() {
                    log_palette_warning("failed to unregister global shortcuts on quit", err);
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = icon {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let bridge_client = BridgeClient::new()
        .map_err(|err| format!("failed to build HTTP client for Labby bridge: {err}"))?;

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_main_window(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            load_palette_config,
            load_palette_default_config,
            save_palette_settings,
            hide_palette,
            show_palette,
            resize_palette,
            toggle_maximize,
            set_blur_dismiss,
            labby_bridge::fetch_catalog,
            labby_bridge::dispatch_action,
            labby_bridge::fetch_launcher_catalog,
            labby_bridge::fetch_launcher_schema,
            labby_bridge::execute_launcher_entry,
            oauth::labby_oauth_login,
            oauth::labby_oauth_logout,
            oauth::labby_oauth_status
        ])
        .manage(BlurDismiss(AtomicBool::new(true)))
        .manage(ActiveShortcut(Mutex::new(None)))
        .manage(bridge_client)
        .manage(oauth::OauthState::new())
        .setup(|app| {
            if let Err(err) = install_tray(app) {
                log_palette_warning("failed to install tray icon", err);
            }
            let settings = merged_settings_or_default(app.handle());
            register_configured_shortcut(app.handle(), &settings).map_err(anyhow::Error::msg)?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let window_handle = handle.clone();
                if let Err(err) = handle.run_on_main_thread(move || {
                    if let Err(err) = show_main_window(&window_handle) {
                        log_palette_warning("failed to show main window on launch", err);
                    }
                }) {
                    log_palette_warning("failed to schedule launch window show", err);
                }
            });
            Ok(())
        })
        .on_window_event(window_events::handle_window_event)
        .run(tauri::generate_context!())
        .map_err(|err| format!("error while running Labby Palette: {err}").into())
}

#[cfg(test)]
mod tests {
    use super::default_server_url;

    #[test]
    fn default_server_url_prefers_dedicated_api_url() {
        assert_eq!(
            default_server_url(
                Some(" http://127.0.0.1:8765/ "),
                Some("https://labby.example.com")
            ),
            "http://127.0.0.1:8765"
        );
    }

    #[test]
    fn default_server_url_falls_back_to_public_url_for_compatibility() {
        assert_eq!(
            default_server_url(None, Some("https://labby.example.com/")),
            "https://labby.example.com"
        );
    }
}
