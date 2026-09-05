//! Surface-neutral identities and content revisions for Labby-owned apps.

use labby_runtime::gateway_config::McpAppsConfig;

pub(crate) const fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

pub(crate) fn bridged_app_content_version(html: &str) -> String {
    let input = format!("{html}\n{}", crate::app_assets::LABBY_APP_HOST_JS);
    format!("{:016x}", fnv1a_64(input.as_bytes()))
}

pub(crate) static CODE_MODE_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        format!(
            "{:016x}",
            fnv1a_64(crate::app_assets::CODE_MODE_APP_HTML.as_bytes())
        )
    });
pub(crate) static SERVER_LOGS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        bridged_app_content_version(crate::app_assets::SERVER_LOGS_APP_HTML)
    });
#[cfg(feature = "skills")]
pub(crate) static SKILL_LIBRARY_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        bridged_app_content_version(crate::app_assets::SKILL_LIBRARY_APP_HTML)
    });
#[cfg(feature = "gateway")]
pub(crate) static ADD_SERVER_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        bridged_app_content_version(crate::app_assets::ADD_SERVER_APP_HTML)
    });
#[cfg(feature = "gateway")]
pub(crate) static GATEWAY_STATUS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        bridged_app_content_version(crate::app_assets::GATEWAY_STATUS_APP_HTML)
    });
#[cfg(feature = "gateway")]
pub(crate) static SETTINGS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bridged_app_content_version(crate::app_assets::SETTINGS_APP_HTML));
#[cfg(feature = "gateway")]
pub(crate) static MCP_APPS_APP_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| bridged_app_content_version(crate::app_assets::MCP_APPS_APP_HTML));

/// Canonical enabled Labby-owned app identities and content revisions.
pub(crate) fn enabled_versions(
    code_mode_enabled: bool,
    config: McpAppsConfig,
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut add = |id: &str, version: &std::sync::LazyLock<String>| {
        rows.push((id.to_owned(), version.to_string()));
    };
    if code_mode_enabled {
        add("code-mode", &CODE_MODE_APP_VERSION);
    }
    if config.server_logs {
        add("server-logs", &SERVER_LOGS_APP_VERSION);
    }
    #[cfg(feature = "skills")]
    add("skill-library", &SKILL_LIBRARY_APP_VERSION);
    #[cfg(feature = "gateway")]
    {
        if config.add_server {
            add("add-server", &ADD_SERVER_APP_VERSION);
        }
        if config.gateway_status {
            add("gateway-status", &GATEWAY_STATUS_APP_VERSION);
        }
        if config.settings {
            add("settings", &SETTINGS_APP_VERSION);
        }
        if config.manager {
            add("mcp-apps", &MCP_APPS_APP_VERSION);
        }
    }
    rows
}
