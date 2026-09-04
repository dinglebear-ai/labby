use std::{
    fmt::Display,
    net::IpAddr,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Size, WebviewUrl, WebviewWindowBuilder,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
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
    control_plane_url: String,
    static_token: Option<String>,
    project_id: Option<String>,
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

/// Process-local settings cache. Disk access happens once on a blocking worker
/// and subsequent bridge commands read this cache without touching the UI
/// thread or Tokio worker threads.
#[derive(Default)]
struct SettingsState(tokio::sync::RwLock<Option<LabbySettings>>);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ControlPlaneLoadPhase {
    #[default]
    Idle,
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Default)]
struct ControlPlaneLoadState {
    generation: u64,
    phase: ControlPlaneLoadPhase,
    target_url: Option<String>,
    shell_ready: bool,
    error_message: Option<String>,
}

#[derive(Default)]
struct ControlPlaneLoad(Mutex<ControlPlaneLoadState>);

impl ControlPlaneLoad {
    fn with_state<T>(&self, f: impl FnOnce(&mut ControlPlaneLoadState) -> T) -> T {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut state)
    }

    fn begin(&self) -> u64 {
        self.with_state(|state| {
            state.generation = state.generation.wrapping_add(1).max(1);
            state.phase = ControlPlaneLoadPhase::Pending;
            state.target_url = None;
            state.shell_ready = false;
            state.error_message = None;
            state.generation
        })
    }

    fn set_target(&self, generation: u64, target_url: &str) -> bool {
        self.with_state(|state| {
            if state.generation != generation || state.phase != ControlPlaneLoadPhase::Pending {
                return false;
            }
            state.target_url = Some(target_url.to_owned());
            state.shell_ready = false;
            true
        })
    }

    fn complete_url(&self, loaded_url: &str) -> bool {
        self.with_state(|state| {
            if state.phase != ControlPlaneLoadPhase::Pending
                || state.target_url.as_deref() != Some(loaded_url)
            {
                return false;
            }
            state.phase = ControlPlaneLoadPhase::Succeeded;
            true
        })
    }

    fn fail(&self, generation: u64) -> bool {
        self.with_state(|state| {
            if state.generation != generation || state.phase != ControlPlaneLoadPhase::Pending {
                return false;
            }
            state.phase = ControlPlaneLoadPhase::Failed;
            true
        })
    }

    fn fail_with_message(&self, generation: u64, message: String) -> bool {
        self.with_state(|state| {
            if state.generation != generation || state.phase != ControlPlaneLoadPhase::Pending {
                return false;
            }
            state.phase = ControlPlaneLoadPhase::Failed;
            state.error_message = Some(message);
            true
        })
    }

    fn ready_error(&self) -> Option<String> {
        self.with_state(|state| {
            (state.shell_ready && state.phase == ControlPlaneLoadPhase::Failed)
                .then(|| state.error_message.clone())
                .flatten()
        })
    }

    fn shell_loaded(&self) -> Option<String> {
        self.with_state(|state| {
            state.shell_ready = true;
            (state.phase == ControlPlaneLoadPhase::Failed)
                .then(|| state.error_message.clone())
                .flatten()
        })
    }

    fn navigation_allowed(&self, url: &tauri::Url) -> bool {
        if is_control_plane_shell_url(url) {
            return true;
        }
        self.with_state(|state| {
            let Some(target) = state.target_url.as_deref() else {
                return false;
            };
            let Ok(target) = tauri::Url::parse(target) else {
                return false;
            };
            matches!(url.scheme(), "http" | "https") && url.origin() == target.origin()
        })
    }
}

fn is_control_plane_shell_url(url: &tauri::Url) -> bool {
    ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost")))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/control-plane-loader.html"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn generation_url(mut url: reqwest::Url, generation: u64) -> reqwest::Url {
    url.set_fragment(Some(&format!("labby-load-generation-{generation}")));
    url
}

fn log_palette_warning(context: &str, err: impl Display) {
    warn(format!("{context}: {err}"));
}

#[tauri::command]
async fn load_palette_config(app: AppHandle) -> Result<LabbySettings, String> {
    merged_settings(&app).await
}

#[tauri::command]
fn load_palette_default_config() -> LabbySettings {
    default_settings()
}

#[tauri::command]
async fn save_palette_settings(
    app: AppHandle,
    settings: LabbySettings,
) -> Result<LabbySettings, String> {
    let settings = normalize_settings(settings);
    // 1. Persist palette-only preferences.
    save_palette_prefs(&app, &settings).await?;
    // 2. Keep the cache consistent with the durable value before applying the
    // fallible OS shortcut side effect.
    *app.state::<SettingsState>().0.write().await = Some(settings.clone());
    // 3. Only mutate runtime state (shortcut) after the write succeeds.
    update_shortcut(&app, &settings)?;
    Ok(settings)
}

async fn save_palette_prefs(app: &AppHandle, settings: &LabbySettings) -> Result<(), String> {
    let app = app.clone();
    let settings = settings.clone();
    persistence::run_blocking_io(move || {
        write_settings(&app, &settings).map_err(|err| err.to_string())
    })
    .await
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

const CONTROL_PLANE_WINDOW: &str = "control-plane";

fn control_plane_url(
    server_url: &str,
    requested_path: Option<&str>,
) -> Result<reqwest::Url, String> {
    let origin = validate_saved_server_url(server_url)?;
    let mut url = reqwest::Url::parse(&origin)
        .map_err(|err| format!("saved Labby server URL is invalid: {err}"))?;
    let requested = requested_path.unwrap_or("/").trim();
    if requested.contains(['\\', '\0', '\r', '\n']) || requested.contains("..") {
        return Err("control plane path is invalid".to_string());
    }
    let requested = if requested.is_empty() { "/" } else { requested };
    if !requested.starts_with('/') || requested.starts_with("//") {
        return Err("control plane path must be an absolute application path".to_string());
    }
    let (path, query) = requested
        .split_once('?')
        .map_or((requested, None), |(path, query)| (path, Some(query)));
    url.set_path(path);
    url.set_query(query.filter(|query| !query.is_empty()));
    url.set_fragment(None);
    Ok(url)
}

fn show_control_plane_window(app: &AppHandle, path: Option<&str>) -> Result<(), String> {
    let load_state = app.state::<ControlPlaneLoad>();
    let generation = load_state.begin();
    if let Err(err) = show_control_plane_shell(app) {
        load_state.fail(generation);
        return Err(err);
    }
    let settings = match merged_settings_from_disk(app) {
        Ok(settings) => settings,
        Err(err) => {
            let message = format!(
                "Unable to load Labby settings: {err}. Open Settings to repair the saved configuration."
            );
            deliver_control_plane_error(app, generation, message.clone());
            return Err(message);
        }
    };
    #[cfg(debug_assertions)]
    let server_url = std::env::var("LABBY_CONTROL_PLANE_DEV_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| settings.control_plane_url.clone());
    #[cfg(not(debug_assertions))]
    let server_url = settings.control_plane_url;
    let url = match control_plane_url(&server_url, path) {
        Ok(url) => url,
        Err(err) => {
            deliver_control_plane_error(app, generation, err.clone());
            return Err(err);
        }
    };
    load_control_plane(app.clone(), generation, url);
    Ok(())
}

fn show_control_plane_shell(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(CONTROL_PLANE_WINDOW) {
        window
            .navigate(
                tauri::Url::parse("tauri://localhost/control-plane-loader.html")
                    .map_err(|err| err.to_string())?,
            )
            .map_err(|err| err.to_string())?;
        window.show().map_err(|err| err.to_string())?;
        window.set_focus().map_err(|err| err.to_string())?;
    } else {
        let navigation_app = app.clone();
        WebviewWindowBuilder::new(
            app,
            CONTROL_PLANE_WINDOW,
            WebviewUrl::App(PathBuf::from("control-plane-loader.html")),
        )
        .title("Labby Control Plane")
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 620.0)
        .resizable(true)
        .center()
        .on_navigation(move |url| {
            navigation_app
                .state::<ControlPlaneLoad>()
                .navigation_allowed(url)
        })
        .on_page_load(|window, payload| {
            if payload.event() != PageLoadEvent::Finished {
                return;
            }
            if is_control_plane_shell_url(payload.url()) {
                if let Some(message) = window.state::<ControlPlaneLoad>().shell_loaded() {
                    render_control_plane_error(&window, &message);
                }
            } else if matches!(payload.url().scheme(), "http" | "https") {
                window
                    .state::<ControlPlaneLoad>()
                    .complete_url(payload.url().as_str());
            }
        })
        .build()
        .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn render_control_plane_error(window: &tauri::WebviewWindow, message: &str) {
    let detail = serde_json::to_string(message)
        .unwrap_or_else(|_| "\"The Control Plane could not be loaded.\"".to_owned());
    if let Err(err) = window.eval(format!("window.showControlPlaneError({detail})")) {
        log_palette_warning("failed to render control-plane error", err);
    }
}

fn deliver_control_plane_error(app: &AppHandle, generation: u64, message: String) {
    let state = app.state::<ControlPlaneLoad>();
    if !state.fail_with_message(generation, message) {
        return;
    }
    if let (Some(message), Some(window)) = (
        state.ready_error(),
        app.get_webview_window(CONTROL_PLANE_WINDOW),
    ) {
        render_control_plane_error(&window, &message);
    }
}

fn restore_control_plane_error_shell(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(CONTROL_PLANE_WINDOW) {
        match window.navigate(
            tauri::Url::parse("tauri://localhost/control-plane-loader.html")
                .expect("static loader URL is valid"),
        ) {
            Ok(()) => return,
            Err(err) => {
                log_palette_warning("failed to restore control-plane error shell", err);
                if let Err(err) = window.destroy() {
                    log_palette_warning("failed to destroy unresponsive control-plane window", err);
                }
            }
        }
    }
    if let Err(err) = show_control_plane_shell(app) {
        log_palette_warning("failed to rebuild control-plane error shell", err);
    }
}

fn load_control_plane(app: AppHandle, generation: u64, url: reqwest::Url) {
    tauri::async_runtime::spawn(async move {
        let origin = url.origin().ascii_serialization();
        let navigation_url = generation_url(url, generation);
        let result = async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|err| err.to_string())?;
            let response = client
                .get(origin.clone())
                .send()
                .await
                .map_err(|err| err.to_string())?;
            if !response.status().is_success() {
                return Err(format!("server returned HTTP {}", response.status()));
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type.starts_with("text/html") {
                return Err(format!(
                    "server returned {content_type} instead of the Labby WebUI"
                ));
            }
            Ok::<_, String>(())
        }
        .await;
        if let Some(window) = app.get_webview_window(CONTROL_PLANE_WINDOW) {
            match result {
                Ok(()) => {
                    let state = app.state::<ControlPlaneLoad>();
                    if !state.set_target(generation, navigation_url.as_str()) {
                        return;
                    }
                    if let Err(err) = window.navigate(navigation_url) {
                        log_palette_warning("failed to navigate control plane", err);
                        if state.fail_with_message(
                            generation,
                            "The Control Plane WebView could not start navigation. Check Settings and retry from the tray."
                                .to_owned(),
                        ) {
                            restore_control_plane_error_shell(&app);
                        }
                        return;
                    }
                    let deadline_app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                        let state = deadline_app.state::<ControlPlaneLoad>();
                        if state.fail_with_message(
                            generation,
                            "The Control Plane did not finish loading within 15 seconds. Check Settings and retry from the tray."
                                .to_owned(),
                        ) {
                            restore_control_plane_error_shell(&deadline_app);
                        }
                    });
                }
                Err(err) => {
                    deliver_control_plane_error(
                        &app,
                        generation,
                        format!(
                            "Could not load {origin}: {err}. Check the Control Plane URL in palette Settings, then retry from the tray."
                        ),
                    );
                }
            }
        }
    });
}

#[tauri::command]
fn open_control_plane(app: AppHandle, path: Option<String>) -> Result<(), String> {
    show_control_plane_window(&app, path.as_deref())
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

fn merged_settings_from_disk(app: &AppHandle) -> Result<LabbySettings, String> {
    let persisted = read_settings_result(app)?;
    let defaults = default_settings();
    Ok(merge_settings(persisted, defaults))
}

async fn merged_settings(app: &AppHandle) -> Result<LabbySettings, String> {
    let state = app.state::<SettingsState>();
    if let Some(settings) = state.0.read().await.clone() {
        return Ok(settings);
    }
    let worker_app = app.clone();
    let loaded =
        persistence::run_blocking_io(move || merged_settings_from_disk(&worker_app)).await?;
    let mut cache = state.0.write().await;
    Ok(cache.get_or_insert_with(|| loaded.clone()).clone())
}

fn merged_settings_or_default(app: &AppHandle) -> LabbySettings {
    match merged_settings_from_disk(app) {
        Ok(settings) => settings,
        Err(err) => {
            warn(&err);
            default_settings()
        }
    }
}

fn merge_settings(persisted: PartialPaletteSettings, defaults: LabbySettings) -> LabbySettings {
    let persisted_server_url = persisted.server_url;
    normalize_settings(LabbySettings {
        server_url: persisted_server_url.clone().unwrap_or(defaults.server_url),
        control_plane_url: persisted
            .control_plane_url
            .or(persisted_server_url)
            .unwrap_or(defaults.control_plane_url),
        static_token: persisted.static_token.unwrap_or(defaults.static_token),
        project_id: persisted.project_id.or(defaults.project_id),
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
    let server_url = default_server_url(value_for("LABBY_API_URL").as_deref());
    let control_plane_url = default_server_url(
        value_for("LABBY_CONTROL_PLANE_URL")
            .as_deref()
            .or(Some(server_url.as_str())),
    );
    let static_token = value_for("LABBY_MCP_HTTP_TOKEN")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let project_id = value_for("LABBY_PROJECT_ID")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    LabbySettings {
        server_url,
        control_plane_url,
        static_token,
        project_id,
        shortcut: DEFAULT_SHORTCUT.to_string(),
        theme: PaletteTheme::System,
        hide_on_blur: true,
        open_results_inline: true,
        show_footer_hints: false,
    }
}

fn default_server_url(api_url: Option<&str>) -> String {
    api_url
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartialPaletteSettings {
    server_url: Option<String>,
    control_plane_url: Option<String>,
    static_token: Option<Option<String>>,
    project_id: Option<String>,
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
    settings.control_plane_url = normalize_server_url(&settings.control_plane_url);
    if settings.control_plane_url.is_empty() {
        settings.control_plane_url = settings.server_url.clone();
    }
    settings.static_token = settings
        .static_token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty());
    settings.project_id = settings
        .project_id
        .map(|project_id| project_id.trim().to_string())
        .filter(|project_id| !project_id.is_empty());
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

/// Validate a saved Labby server URL. Cleartext HTTP is limited to loopback so
/// credentials and authenticated Control Plane pages cannot cross the network
/// without transport security.
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
    if parsed.scheme() == "http" && !is_loopback_host(parsed.host_str().unwrap_or_default()) {
        return Err(
            "saved Labby server URL must use https unless the host is loopback".to_string(),
        );
    }
    Ok(server_url)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
    let control_plane = MenuItem::with_id(
        app,
        "control-plane",
        "Open Control Plane",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Labby Palette", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &control_plane, &settings, &quit])?;

    let icon = app.default_window_icon().cloned();
    let mut tray = TrayIconBuilder::new()
        .tooltip("Labby")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Err(err) = show_main_window(app) {
                    log_palette_warning("failed to show main window from tray", err);
                }
            }
            "control-plane" => {
                if let Err(err) = show_control_plane_window(app, None) {
                    log_palette_warning("failed to show control plane from tray", err);
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
            open_control_plane,
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
        .manage(SettingsState::default())
        .manage(ControlPlaneLoad::default())
        .manage(bridge_client)
        .manage(oauth::OauthState::new())
        .setup(|app| {
            if let Err(err) = install_tray(app) {
                log_palette_warning("failed to install tray icon", err);
            }
            let settings = merged_settings_or_default(app.handle());
            if let Ok(mut cache) = app.state::<SettingsState>().0.try_write() {
                *cache = Some(settings.clone());
            }
            register_configured_shortcut(app.handle(), &settings).map_err(anyhow::Error::msg)?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let window_handle = handle.clone();
                if let Err(err) = handle.run_on_main_thread(move || {
                    if let Err(err) = show_control_plane_window(&window_handle, None) {
                        log_palette_warning("failed to show control plane on launch", err);
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
    use super::{
        ControlPlaneLoad, LabbySettings, PaletteTheme, PartialPaletteSettings, control_plane_url,
        default_server_url, generation_url, merge_settings, normalize_settings,
        validate_saved_server_url,
    };

    #[test]
    fn default_server_url_prefers_dedicated_api_url() {
        assert_eq!(
            default_server_url(Some(" http://127.0.0.1:8765/ ")),
            "http://127.0.0.1:8765"
        );
    }

    #[test]
    fn default_server_url_does_not_use_oauth_public_url() {
        assert_eq!(default_server_url(None), "http://localhost:8765");
    }
    #[test]
    fn control_plane_origin_can_differ_and_legacy_settings_follow_server() {
        let defaults = LabbySettings {
            server_url: "https://api.default.example".to_owned(),
            control_plane_url: "https://ui.default.example".to_owned(),
            static_token: None,
            project_id: None,
            shortcut: "Ctrl+Shift+Space".to_owned(),
            theme: PaletteTheme::System,
            hide_on_blur: true,
            open_results_inline: true,
            show_footer_hints: false,
        };
        let split = merge_settings(
            PartialPaletteSettings {
                server_url: Some("https://api.example".to_owned()),
                control_plane_url: Some("https://ui.example".to_owned()),
                ..Default::default()
            },
            defaults.clone(),
        );
        assert_eq!(split.server_url, "https://api.example");
        assert_eq!(split.control_plane_url, "https://ui.example");

        let legacy = merge_settings(
            PartialPaletteSettings {
                server_url: Some("https://legacy.example".to_owned()),
                ..Default::default()
            },
            defaults,
        );
        assert_eq!(legacy.control_plane_url, "https://legacy.example");
    }

    #[test]
    fn control_plane_url_preserves_only_internal_path_and_query() {
        assert_eq!(
            control_plane_url(
                "https://labby.example.com/mcp",
                Some("/skills/?tab=library"),
            )
            .unwrap()
            .as_str(),
            "https://labby.example.com/skills/?tab=library"
        );
    }

    #[test]
    fn control_plane_url_rejects_cross_origin_and_traversal_paths() {
        for path in [
            "https://attacker.invalid/",
            "//attacker.invalid/",
            "/../authorize",
            "/skills\\evil",
        ] {
            assert!(control_plane_url("https://labby.example.com", Some(path)).is_err());
        }
    }

    #[test]
    fn saved_server_url_requires_tls_outside_loopback() {
        for allowed in [
            "https://labby.example.com",
            "http://localhost:8765",
            "https://localhost:8765",
            "http://127.0.0.1:8765",
            "https://127.0.0.1:8765",
            "http://[::1]:8765",
            "https://[::1]:8765",
        ] {
            assert_eq!(validate_saved_server_url(allowed).unwrap(), allowed);
        }

        for rejected in ["http://labby.example.com", "http://192.168.1.20:8765"] {
            assert!(
                validate_saved_server_url(rejected).is_err(),
                "{rejected} must not be accepted over cleartext HTTP"
            );
        }
    }

    #[test]
    fn control_plane_load_is_latest_wins() {
        let load = ControlPlaneLoad::default();
        let first = load.begin();
        let first_url = generation_url(
            tauri::Url::parse("https://labby.example.com/skills/").unwrap(),
            first,
        );
        assert!(load.set_target(first, first_url.as_str()));

        let second = load.begin();
        let second_url = generation_url(
            tauri::Url::parse("https://labby.example.com/skills/").unwrap(),
            second,
        );
        assert!(load.set_target(second, second_url.as_str()));
        assert!(!load.complete_url(first_url.as_str()));
        assert!(!load.fail(first));
        assert!(load.complete_url(second_url.as_str()));
        assert!(!load.fail(second));
    }

    #[test]
    fn control_plane_load_arbitrates_failure_and_retry() {
        let load = ControlPlaneLoad::default();
        let timed_out = load.begin();
        assert!(load.set_target(timed_out, "https://labby.example.com/skills/"));
        assert!(load.fail(timed_out));
        assert!(!load.complete_url("https://labby.example.com/skills/"));

        let retry = load.begin();
        assert!(!load.fail(timed_out));
        assert!(load.set_target(retry, "https://labby.example.com/tools/"));
        assert!(load.complete_url("https://labby.example.com/tools/"));
    }

    #[test]
    fn control_plane_error_waits_for_ready_shell() {
        let load = ControlPlaneLoad::default();
        let generation = load.begin();
        assert!(load.fail_with_message(generation, "connection failed".to_owned()));
        assert_eq!(load.ready_error(), None);
        assert_eq!(load.shell_loaded().as_deref(), Some("connection failed"));
        assert_eq!(load.ready_error().as_deref(), Some("connection failed"));
    }

    #[test]
    fn control_plane_navigation_is_confined_to_target_origin_and_error_shell() {
        let load = ControlPlaneLoad::default();
        let shell = tauri::Url::parse("tauri://localhost/control-plane-loader.html").unwrap();
        let target = tauri::Url::parse("https://labby.example.com/skills/").unwrap();
        let same_origin = tauri::Url::parse("https://labby.example.com/settings/").unwrap();
        let attacker = tauri::Url::parse("https://attacker.invalid/phish").unwrap();
        let downgraded = tauri::Url::parse("http://labby.example.com/skills/").unwrap();
        let fake_shell =
            tauri::Url::parse("http://localhost:9999/control-plane-loader.html").unwrap();
        let ported_shell =
            tauri::Url::parse("http://tauri.localhost:9999/control-plane-loader.html").unwrap();

        assert!(load.navigation_allowed(&shell));
        assert!(!load.navigation_allowed(&target));
        let generation = load.begin();
        assert!(load.set_target(generation, target.as_str()));
        assert!(load.navigation_allowed(&target));
        assert!(load.navigation_allowed(&same_origin));
        assert!(!load.navigation_allowed(&attacker));
        assert!(!load.navigation_allowed(&downgraded));
        assert!(!load.navigation_allowed(&fake_shell));
        assert!(!load.navigation_allowed(&ported_shell));
    }

    #[test]
    fn project_context_is_trimmed_and_empty_values_are_removed() {
        let settings = |project_id| LabbySettings {
            server_url: "http://localhost:8765".to_string(),
            control_plane_url: "http://localhost:8765".to_string(),
            static_token: None,
            project_id,
            shortcut: "Ctrl+Shift+Space".to_string(),
            theme: PaletteTheme::System,
            hide_on_blur: true,
            open_results_inline: true,
            show_footer_hints: false,
        };

        assert_eq!(
            normalize_settings(settings(Some(" team-project ".to_string()))).project_id,
            Some("team-project".to_string())
        );
        assert_eq!(
            normalize_settings(settings(Some("   ".to_string()))).project_id,
            None
        );
    }
}
