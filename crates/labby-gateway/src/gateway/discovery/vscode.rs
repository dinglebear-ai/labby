use std::path::{Path, PathBuf};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use super::xdg_config_home;
use super::{DiscoveredServer, scan_paths};

/// Scans VS Code MCP configs. Also covers GitHub Copilot, which uses VS Code's
/// mcp.json when running as a VS Code extension.
pub fn discover(home: &Path) -> Vec<DiscoveredServer> {
    discover_with_appdata(
        home,
        std::env::var_os("APPDATA").map(PathBuf::from).as_deref(),
    )
}

pub(super) fn discover_with_appdata(home: &Path, appdata: Option<&Path>) -> Vec<DiscoveredServer> {
    #[cfg(not(windows))]
    let _ = appdata;
    let mut paths: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        paths.push(home.join("Library/Application Support/Code/User/mcp.json"));
        paths.push(home.join("Library/Application Support/Code - Insiders/User/mcp.json"));
        paths.push(home.join("Library/Application Support/Antigravity/User/mcp.json"));
    }

    #[cfg(target_os = "windows")]
    {
        let appdata = appdata
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData/Roaming"));
        paths.push(appdata.join("Code/User/mcp.json"));
        paths.push(appdata.join("Code - Insiders/User/mcp.json"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let xdg = xdg_config_home(home);
        for config_root in [xdg, home.join(".config")] {
            if paths.iter().any(|path| path.starts_with(&config_root)) {
                continue;
            }
            paths.push(config_root.join("Code/User/mcp.json"));
            paths.push(config_root.join("Code - Insiders/User/mcp.json"));
            paths.push(config_root.join("Antigravity/User/mcp.json"));
        }
    }

    scan_paths(&paths, "vscode", true)
}
