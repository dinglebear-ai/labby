//! Native Labby Control Plane desktop shell.
//!
//! The hosted Control Plane owns UI, authentication, commands, and product
//! settings. This crate owns only its native window, credential-free bootstrap
//! origin, confined navigation, and platform lifecycle.

use std::{fmt::Display, net::IpAddr, sync::Mutex};

use tauri::{
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
};

mod desktop_auth;
mod persistence;

use persistence::{BootstrapSettings, load_bootstrap_settings};

const CONTROL_PLANE_WINDOW: &str = "control-plane";
const CONTROL_PLANE_LOADER: &str = "control-plane-loader.html";
const DEFAULT_CONTROL_PLANE_URL: &str = "http://localhost:8765";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    OpenControlPlane,
    OpenSettings,
    Quit,
}

fn menu_action(id: &str) -> Option<MenuAction> {
    match id {
        "open-control-plane" => Some(MenuAction::OpenControlPlane),
        "open-settings" => Some(MenuAction::OpenSettings),
        "quit" => Some(MenuAction::Quit),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum LoadPhase {
    #[default]
    Idle,
    Pending,
    Succeeded,
    Failed,
}

#[derive(Debug, Default)]
struct LoadState {
    generation: u64,
    phase: LoadPhase,
    target_url: Option<String>,
    loader_ready: bool,
    error_message: Option<String>,
}

#[derive(Default)]
struct ControlPlaneLoad(Mutex<LoadState>);

impl ControlPlaneLoad {
    fn with_state<T>(&self, f: impl FnOnce(&mut LoadState) -> T) -> T {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut state)
    }

    fn begin(&self) -> u64 {
        self.with_state(|state| {
            state.generation = state.generation.wrapping_add(1).max(1);
            state.phase = LoadPhase::Pending;
            state.target_url = None;
            state.loader_ready = false;
            state.error_message = None;
            state.generation
        })
    }

    fn set_target(&self, generation: u64, target_url: &str) -> bool {
        self.with_state(|state| {
            if state.generation != generation || state.phase != LoadPhase::Pending {
                return false;
            }
            state.target_url = Some(target_url.to_owned());
            true
        })
    }

    fn complete_url(&self, loaded_url: &str) -> bool {
        self.with_state(|state| {
            if state.phase != LoadPhase::Pending {
                return false;
            }
            let Some(target) = state
                .target_url
                .as_deref()
                .and_then(|value| tauri::Url::parse(value).ok())
            else {
                return false;
            };
            let Ok(loaded) = tauri::Url::parse(loaded_url) else {
                return false;
            };
            if loaded.origin() != target.origin() {
                return false;
            }
            if loaded
                .fragment()
                .is_some_and(|fragment| fragment.starts_with("labby-load-generation-"))
                && loaded.fragment() != target.fragment()
            {
                return false;
            }
            state.phase = LoadPhase::Succeeded;
            true
        })
    }

    fn fail_with_message(&self, generation: u64, message: String) -> bool {
        self.with_state(|state| {
            if state.generation != generation || state.phase != LoadPhase::Pending {
                return false;
            }
            state.phase = LoadPhase::Failed;
            state.error_message = Some(message);
            true
        })
    }

    fn loader_loaded(&self) -> Option<String> {
        self.with_state(|state| {
            state.loader_ready = true;
            (state.phase == LoadPhase::Failed)
                .then(|| state.error_message.clone())
                .flatten()
        })
    }

    fn ready_error(&self) -> Option<String> {
        self.with_state(|state| {
            (state.loader_ready && state.phase == LoadPhase::Failed)
                .then(|| state.error_message.clone())
                .flatten()
        })
    }

    fn navigation_allowed(&self, url: &tauri::Url) -> bool {
        if is_loader_url(url) {
            return true;
        }
        self.with_state(|state| {
            let Some(target) = state.target_url.as_deref() else {
                return false;
            };
            let Ok(target) = tauri::Url::parse(target) else {
                return false;
            };
            (matches!(url.scheme(), "http" | "https") && url.origin() == target.origin())
        })
    }
}

fn warn(context: &str, error: impl Display) {
    tracing::warn!("{context}: {error}");
}

fn is_loader_url(url: &tauri::Url) -> bool {
    ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost")))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == format!("/{CONTROL_PLANE_LOADER}")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn bundled_loader_url(uses_http_protocol: bool) -> tauri::Url {
    // Match Tauri's internal tauri_protocol_url(false): WebView2/Android
    // serve custom protocols through an HTTP authority. Explicit navigate()
    // does not perform the initial WebviewUrl::App platform conversion.
    let base = if uses_http_protocol {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    tauri::Url::parse(&format!("{base}/{CONTROL_PLANE_LOADER}")).expect("static bundled loader URL")
}

fn loader_url() -> tauri::Url {
    bundled_loader_url(cfg!(any(windows, target_os = "android")))
}

fn generation_url(mut url: reqwest::Url, generation: u64) -> reqwest::Url {
    url.set_fragment(Some(&format!("labby-load-generation-{generation}")));
    url
}

fn normalize_origin(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
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

fn validate_control_plane_origin(value: &str) -> Result<String, String> {
    let origin = normalize_origin(value);
    if origin.is_empty() {
        return Err("no Labby Control Plane URL is configured".to_owned());
    }
    let parsed = reqwest::Url::parse(&origin)
        .map_err(|error| format!("saved Labby Control Plane URL is invalid: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("saved Labby Control Plane URL must be an HTTP(S) origin".to_owned());
    }
    if parsed.scheme() == "http" && !is_loopback_host(parsed.host_str().unwrap_or_default()) {
        return Err(
            "saved Labby Control Plane URL must use HTTPS unless it is loopback".to_owned(),
        );
    }
    Ok(origin)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn control_plane_url(origin: &str, requested_path: Option<&str>) -> Result<reqwest::Url, String> {
    let origin = validate_control_plane_origin(origin)?;
    let mut url = reqwest::Url::parse(&origin)
        .map_err(|error| format!("saved Labby Control Plane URL is invalid: {error}"))?;
    let requested = requested_path.unwrap_or("/").trim();
    if requested.contains(['\\', '\0', '\r', '\n']) || requested.contains("..") {
        return Err("Control Plane path is invalid".to_owned());
    }
    let requested = if requested.is_empty() { "/" } else { requested };
    if !requested.starts_with('/') || requested.starts_with("//") {
        return Err("Control Plane path must be an absolute application path".to_owned());
    }
    let (path, query) = requested
        .split_once('?')
        .map_or((requested, None), |(path, query)| (path, Some(query)));
    url.set_path(path);
    url.set_query(query.filter(|query| !query.is_empty()));
    url.set_fragment(None);
    Ok(url)
}

fn configured_origin(app: &AppHandle) -> Result<String, String> {
    #[cfg(debug_assertions)]
    if let Some(value) = persistence::value_for("LABBY_CONTROL_PLANE_DEV_URL") {
        return validate_control_plane_origin(&value);
    }
    let BootstrapSettings { control_plane_url } = load_bootstrap_settings(app)?;
    validate_control_plane_origin(&control_plane_url)
}

fn build_control_plane_window(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(CONTROL_PLANE_WINDOW).is_some() {
        return Ok(());
    }
    let navigation_app = app.clone();
    WebviewWindowBuilder::new(
        app,
        CONTROL_PLANE_WINDOW,
        // Keep the fallback bundled even in dev mode, where WebviewUrl::App
        // would instead resolve to the (intentionally untrusted) dev server.
        WebviewUrl::CustomProtocol(loader_url()),
    )
    .title("Labby Control Plane")
    .inner_size(1280.0, 820.0)
    .min_inner_size(900.0, 620.0)
    .decorations(true)
    .resizable(true)
    .visible(true)
    .center()
    .on_navigation(move |url| {
        let origin = configured_origin(&navigation_app).ok();
        if let Some(return_to) = origin
            .as_deref()
            .and_then(|origin| desktop_auth::login_return_to(url, origin))
        {
            desktop_auth::begin(navigation_app.clone(), return_to);
            return false;
        }
        navigation_app
            .state::<desktop_auth::DesktopAuthState>()
            .cancel();
        navigation_app
            .state::<ControlPlaneLoad>()
            .navigation_allowed(url)
    })
    .on_page_load(|window, payload| {
        if payload.event() != PageLoadEvent::Finished {
            return;
        }
        if is_loader_url(payload.url()) {
            if let Some(message) = window.state::<ControlPlaneLoad>().loader_loaded() {
                render_error(&window, &message);
            }
        } else if matches!(payload.url().scheme(), "http" | "https") {
            window
                .state::<ControlPlaneLoad>()
                .complete_url(payload.url().as_str());
        }
    })
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn show_loader(app: &AppHandle) -> Result<(), String> {
    build_control_plane_window(app)?;
    let window = app
        .get_webview_window(CONTROL_PLANE_WINDOW)
        .ok_or_else(|| "Control Plane window not found".to_owned())?;
    window
        .navigate(loader_url())
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

fn render_error(window: &tauri::WebviewWindow, message: &str) {
    let detail = serde_json::to_string(message)
        .unwrap_or_else(|_| "\"The Control Plane could not be loaded.\"".to_owned());
    if let Err(error) = window.eval(format!("window.showControlPlaneError({detail})")) {
        warn("failed to render Control Plane error", error);
    }
}

fn deliver_error(app: &AppHandle, generation: u64, message: String) {
    let state = app.state::<ControlPlaneLoad>();
    if !state.fail_with_message(generation, message) {
        return;
    }
    if let (Some(message), Some(window)) = (
        state.ready_error(),
        app.get_webview_window(CONTROL_PLANE_WINDOW),
    ) {
        render_error(&window, &message);
    }
}

fn restore_error_loader(app: &AppHandle) {
    if let Err(error) = show_loader(app) {
        warn("failed to restore Control Plane loader", error);
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
                .map_err(|error| error.to_string())?;
            client
                .get(&origin)
                .send()
                .await
                .map_err(|error| error.to_string())?;
            // HTTP errors and redirects can be the hosted application's auth
            // boundary. Network reachability is all the native shell should
            // decide; the WebView-owned session handles the actual response.
            Ok::<_, String>(())
        }
        .await;

        let Some(window) = app.get_webview_window(CONTROL_PLANE_WINDOW) else {
            return;
        };
        match result {
            Ok(()) => {
                let state = app.state::<ControlPlaneLoad>();
                if !state.set_target(generation, navigation_url.as_str()) {
                    return;
                }
                if let Err(error) = window.navigate(navigation_url) {
                    if state.fail_with_message(
                        generation,
                        "The Control Plane WebView could not start navigation. Use the Labby menu to retry."
                            .to_owned(),
                    ) {
                        restore_error_loader(&app);
                    }
                    warn("failed to navigate Control Plane", error);
                    return;
                }
                let deadline_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    let state = deadline_app.state::<ControlPlaneLoad>();
                    if state.fail_with_message(
                        generation,
                        "The Control Plane did not finish loading within 15 seconds. Use the Labby menu to retry."
                            .to_owned(),
                    ) {
                        restore_error_loader(&deadline_app);
                    }
                });
            }
            Err(error) => deliver_error(
                &app,
                generation,
                format!("Could not load {origin}: {error}. Use the Labby menu to retry."),
            ),
        }
    });
}

fn show_control_plane(app: &AppHandle, path: Option<&str>) -> Result<(), String> {
    let generation = app.state::<ControlPlaneLoad>().begin();
    show_loader(app)?;
    let origin = match configured_origin(app) {
        Ok(origin) => origin,
        Err(error) => {
            deliver_error(
                app,
                generation,
                format!("{error}. Use the Labby menu to retry."),
            );
            return Ok(());
        }
    };
    let url = match control_plane_url(&origin, path) {
        Ok(url) => url,
        Err(error) => {
            deliver_error(
                app,
                generation,
                format!("{error}. Use the Labby menu to retry."),
            );
            return Ok(());
        }
    };
    load_control_plane(app.clone(), generation, url);
    Ok(())
}

fn restore_control_plane(app: &AppHandle) -> Result<(), String> {
    let phase = app
        .state::<ControlPlaneLoad>()
        .with_state(|state| state.phase);
    if matches!(phase, LoadPhase::Pending | LoadPhase::Succeeded)
        && let Some(window) = app.get_webview_window(CONTROL_PLANE_WINDOW)
    {
        window.unminimize().map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        return window.set_focus().map_err(|error| error.to_string());
    }
    show_control_plane(app, None)
}

#[tauri::command]
fn open_control_plane(app: AppHandle, path: Option<String>) -> Result<(), String> {
    show_control_plane(&app, path.as_deref())
}

fn handle_menu_action(app: &AppHandle, id: &str) {
    let result = match menu_action(id) {
        Some(MenuAction::OpenControlPlane) => restore_control_plane(app),
        Some(MenuAction::OpenSettings) => show_control_plane(app, Some("/settings/")),
        Some(MenuAction::Quit) => {
            app.exit(0);
            Ok(())
        }
        None => Ok(()),
    };
    if let Err(error) = result {
        warn("failed to handle Labby menu action", error);
    }
}

fn shell_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(
        app,
        "open-control-plane",
        "Open Control Plane",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "open-settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Labby", true, None::<&str>)?;
    Menu::with_items(app, &[&open, &settings, &quit])
}

fn install_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(
        app,
        "open-control-plane",
        "Open Control Plane",
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "open-settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Labby", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &settings, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Labby")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu_action(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && let Err(error) = restore_control_plane(tray.app_handle())
            {
                warn("failed to open Control Plane from tray", error);
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() == CONTROL_PLANE_WINDOW
        && let tauri::WindowEvent::CloseRequested { api, .. } = event
    {
        api.prevent_close();
        if let Err(error) = window.hide() {
            warn("failed to hide Control Plane window on close", error);
        }
    }
}

#[cfg(target_os = "macos")]
fn handle_run_event(app: &AppHandle, event: RunEvent) {
    if let RunEvent::Reopen { .. } = event
        && let Err(error) = restore_control_plane(app)
    {
        warn("failed to reopen Control Plane", error);
    }
}

#[cfg(not(target_os = "macos"))]
fn handle_run_event(_app: &AppHandle, _event: RunEvent) {}

/// Run the Labby desktop Control Plane shell.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let app = tauri::Builder::default()
        .menu(shell_menu)
        .on_menu_event(|app, event| handle_menu_action(app, event.id().as_ref()))
        .invoke_handler(tauri::generate_handler![open_control_plane])
        .manage(ControlPlaneLoad::default())
        .manage(desktop_auth::DesktopAuthState::default())
        .setup(|app| {
            install_tray(app)?;
            Ok(show_control_plane(app.handle(), None).map_err(anyhow::Error::msg)?)
        })
        .on_window_event(handle_window_event)
        .build(tauri::generate_context!())?;
    app.run(handle_run_event);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_config_declares_no_second_automatic_window() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["app"]["windows"].as_array().map(Vec::len), Some(0));
        assert!(!config["app"]["macOSPrivateApi"].as_bool().unwrap_or(false));
    }

    #[test]
    fn bundled_loader_uses_the_platform_custom_protocol_authority() {
        for (uses_http, expected) in [
            (false, "tauri://localhost/control-plane-loader.html"),
            (true, "http://tauri.localhost/control-plane-loader.html"),
        ] {
            let url = bundled_loader_url(uses_http);
            assert_eq!(url.as_str(), expected);
            assert!(is_loader_url(&url));
            assert!(ControlPlaneLoad::default().navigation_allowed(&url));
        }
    }

    #[test]
    fn native_manifest_has_no_palette_or_credential_runtime() {
        let manifest = include_str!("../Cargo.toml");
        for retired in ["palette", "keyring", "global-shortcut", "uuid"] {
            assert!(
                !manifest.contains(retired),
                "retired native dependency or identity remains: {retired}"
            );
        }
    }

    #[test]
    fn menu_vocabulary_is_control_plane_only() {
        assert_eq!(
            menu_action("open-control-plane"),
            Some(MenuAction::OpenControlPlane)
        );
        assert_eq!(menu_action("open-settings"), Some(MenuAction::OpenSettings));
        assert_eq!(menu_action("quit"), Some(MenuAction::Quit));
        assert_eq!(menu_action("show-palette"), None);
    }

    #[test]
    fn control_plane_url_preserves_only_internal_path_and_query() {
        assert_eq!(
            control_plane_url(
                "https://labby.example.com/mcp",
                Some("/skills/?tab=library")
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
    fn control_plane_origin_requires_tls_outside_loopback() {
        for allowed in [
            "https://labby.example.com",
            "http://localhost:8765",
            "http://127.0.0.1:8765",
            "http://[::1]:8765",
        ] {
            assert_eq!(validate_control_plane_origin(allowed).unwrap(), allowed);
        }
        for rejected in ["http://labby.example.com", "http://192.168.1.20:8765"] {
            assert!(validate_control_plane_origin(rejected).is_err());
        }
    }

    #[test]
    fn control_plane_navigation_is_confined_to_target_origin_and_loader() {
        let load = ControlPlaneLoad::default();
        let loader = tauri::Url::parse("tauri://localhost/control-plane-loader.html").unwrap();
        let target = tauri::Url::parse("https://labby.example.com/").unwrap();
        assert!(load.navigation_allowed(&loader));
        assert!(!load.navigation_allowed(&target));
        let generation = load.begin();
        assert!(load.set_target(generation, target.as_str()));
        assert!(load.navigation_allowed(&target));
        assert!(load.navigation_allowed(
            &tauri::Url::parse("https://labby.example.com/settings/").unwrap()
        ));
        assert!(
            !load.navigation_allowed(
                &tauri::Url::parse("https://accounts.google.com/o/oauth2/v2/auth?client_id=test")
                    .unwrap()
            )
        );
        assert!(!load.navigation_allowed(
            &tauri::Url::parse("http://accounts.google.com/o/oauth2/v2/auth").unwrap()
        ));
        assert!(!load.navigation_allowed(
            &tauri::Url::parse("https://accounts.google.com.attacker.invalid/").unwrap()
        ));
        assert!(!load.navigation_allowed(&tauri::Url::parse("https://attacker.invalid/").unwrap()));
    }

    #[test]
    fn latest_load_wins_and_offline_error_waits_for_loader() {
        let load = ControlPlaneLoad::default();
        let first = load.begin();
        assert!(load.set_target(first, "https://labby.example.com/#first"));
        let second = load.begin();
        assert!(!load.complete_url("https://labby.example.com/#first"));
        assert!(load.fail_with_message(second, "offline".to_owned()));
        assert_eq!(load.ready_error(), None);
        assert_eq!(load.loader_loaded().as_deref(), Some("offline"));
    }

    #[test]
    fn same_origin_redirect_completes_current_load_but_old_generation_does_not() {
        let load = ControlPlaneLoad::default();
        let first = load.begin();
        assert!(load.set_target(first, "https://labby.example.com/#labby-load-generation-1"));
        let second = load.begin();
        assert!(load.set_target(
            second,
            "https://labby.example.com/settings/#labby-load-generation-2"
        ));
        assert!(!load.complete_url("https://labby.example.com/#labby-load-generation-1"));
        assert!(load.complete_url("https://labby.example.com/auth/login"));
    }
}
