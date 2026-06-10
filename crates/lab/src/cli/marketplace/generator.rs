use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;

use crate::output::OutputFormat;
use crate::output::theme::CliTheme;
use crate::registry::{build_default_registry, service_meta};

const DEFAULT_ORG: &str = match option_env!("LAB_PLUGIN_ORG") {
    Some(value) => value,
    None => "lab",
};
const CORE_PLUGIN: &str = "lab-core";
/// Service plugins invoke labby from PATH. The plugin tree deliberately does
/// NOT bundle the binary — hosts install labby explicitly (scripts/install.sh,
/// a GitHub release archive, or `cargo install`) and `labby setup` owns the
/// rest of the flow.
const CORE_BINARY_COMMAND: &str = "labby";
const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh";

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Output directory for the generated marketplace tree.
    #[arg(long)]
    pub out: PathBuf,
    /// Marketplace/org suffix used in plugin ids, for example `lab`.
    #[arg(long, default_value = DEFAULT_ORG)]
    pub org: String,
}

pub fn run_generate(args: GenerateArgs, format: OutputFormat) -> Result<ExitCode> {
    let theme = CliTheme::from_context(format.render_context());
    generate_marketplace(&args.out, &args.org)?;
    println!(
        "{} {}",
        theme.muted("generated marketplace at"),
        theme.primary(&args.out.display().to_string())
    );
    Ok(ExitCode::SUCCESS)
}

fn generate_marketplace(out: &Path, org: &str) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;

    let registry = build_default_registry();
    let mut service_names = registry
        .services()
        .iter()
        .filter_map(|entry| service_meta(entry.name).map(|_| entry.name.to_string()))
        .collect::<Vec<_>>();
    service_names.sort();

    write_core_plugin(out, org)?;
    for service in &service_names {
        write_service_plugin(out, org, service)?;
    }
    write_marketplace_manifest(out, org, &service_names)?;
    Ok(())
}

fn write_core_plugin(out: &Path, org: &str) -> Result<()> {
    let root = out.join(CORE_PLUGIN);
    fs::create_dir_all(root.join(".claude-plugin"))?;
    fs::create_dir_all(root.join("commands"))?;
    fs::create_dir_all(root.join("skills/install-labby"))?;

    let manifest = plugin_manifest(
        CORE_PLUGIN,
        "Setup commands and skills for Claude Code Labby service plugins.",
        org,
        &["labby", "setup", "homelab", "mcp"],
    );
    write_json(&root.join(".claude-plugin/plugin.json"), &manifest)?;
    write_json(&root.join("plugin.json"), &manifest)?;
    fs::write(root.join("README.md"), core_readme(org))?;
    fs::write(
        root.join("commands/setup-core.md"),
        setup_core_command(false),
    )?;
    fs::write(
        root.join("commands/setup-core-advanced.md"),
        setup_core_command(true),
    )?;
    fs::write(
        root.join("skills/install-labby/SKILL.md"),
        install_labby_skill(),
    )?;
    Ok(())
}

fn write_service_plugin(out: &Path, org: &str, service: &str) -> Result<()> {
    let Some(meta) = service_meta(service) else {
        return Ok(());
    };
    let plugin_name = format!("lab-{service}");
    let root = out.join(&plugin_name);
    fs::create_dir_all(root.join(".claude-plugin"))?;
    fs::create_dir_all(root.join("commands"))?;

    let manifest = plugin_manifest(
        &plugin_name,
        meta.description,
        org,
        &[service, "labby", "mcp", "homelab"],
    );
    write_json(&root.join(".claude-plugin/plugin.json"), &manifest)?;
    write_json(&root.join("plugin.json"), &manifest)?;
    write_json(
        &root.join(".mcp.json"),
        &json!({
            "mcpServers": {
                service: {
                    "command": CORE_BINARY_COMMAND,
                    "args": ["mcp", "--services", service]
                }
            }
        }),
    )?;
    fs::write(root.join("README.md"), service_readme(service, org))?;
    fs::write(
        root.join("commands/install-core.md"),
        install_core_command(org),
    )?;
    Ok(())
}

fn write_marketplace_manifest(out: &Path, org: &str, services: &[String]) -> Result<()> {
    let mut plugins = Vec::with_capacity(services.len() + 1);
    plugins.push(json!({
        "name": CORE_PLUGIN,
        "source": format!("./{CORE_PLUGIN}"),
        "description": "Setup commands and skills for Claude Code Labby service plugins."
    }));
    for service in services {
        let Some(meta) = service_meta(service) else {
            continue;
        };
        plugins.push(json!({
            "name": format!("lab-{service}"),
            "source": format!("./lab-{service}"),
            "description": meta.description
        }));
    }
    let manifest = json!({
        "$schema": "https://json.schemastore.org/claude-code-marketplace.json",
        "name": org,
        "owner": {
            "name": "Labby",
            "email": "noreply@example.invalid"
        },
        "description": "Generated Labby Claude Code service plugins.",
        "plugins": plugins
    });
    write_json(&out.join("plugin-marketplace.json"), &manifest)?;
    fs::create_dir_all(out.join(".claude-plugin"))?;
    write_json(&out.join(".claude-plugin/marketplace.json"), &manifest)?;
    Ok(())
}

fn plugin_manifest(
    name: &str,
    description: &str,
    org: &str,
    keywords: &[&str],
) -> serde_json::Value {
    json!({
        "$schema": "https://json.schemastore.org/claude-code-plugin-manifest.json",
        "name": name,
        "description": description,
        "author": {
            "name": "Labby",
            "email": "noreply@example.invalid"
        },
        "repository": "https://github.com/jmagar/lab",
        "homepage": "https://github.com/jmagar/lab",
        "license": "MIT OR Apache-2.0",
        "keywords": keywords,
        "metadata": {
            "marketplace": org
        }
    })
}

fn core_readme(org: &str) -> String {
    format!(
        "# lab-core\n\nCore Labby plugin for Claude Code.\n\nThis plugin does **not** bundle the `labby` binary. Install it first:\n\n```bash\ncurl -fsSL {INSTALL_SCRIPT_URL} | sh\n# or: cargo install --git https://github.com/jmagar/lab --bin labby --all-features\n```\n\nthen run `labby setup`.\n\nCommands:\n\n- `/setup-core` runs `labby setup --mode plugin` for the plugin-focused setup flow.\n- `/setup-core-advanced` runs `labby setup --mode full` for the full operator setup flow.\n\nService plugins invoke `labby` from PATH.\n\nInstall service plugins as `lab-<service>@{org}` after installing this core plugin.\n"
    )
}

fn service_readme(service: &str, org: &str) -> String {
    let Some(meta) = service_meta(service) else {
        return String::new();
    };
    let required = if meta.required_env.is_empty() {
        "- none\n".to_string()
    } else {
        meta.required_env
            .iter()
            .map(|var| format!("- `{}` - {}\n", var.name, var.description))
            .collect::<String>()
    };
    let optional = if meta.optional_env.is_empty() {
        "- none\n".to_string()
    } else {
        meta.optional_env
            .iter()
            .map(|var| format!("- `{}` - {}\n", var.name, var.description))
            .collect::<String>()
    };
    format!(
        "# lab-{service}\n\n{}\n\nThis plugin starts Labby with only `{service}` enabled:\n\n```json\n{{ \"command\": \"{CORE_BINARY_COMMAND}\", \"args\": [\"mcp\", \"--services\", \"{service}\"] }}\n```\n\n`labby` must be installed on PATH first:\n\n```bash\ncurl -fsSL {INSTALL_SCRIPT_URL} | sh\n```\n\nRun `/setup-core` to fill in service credentials.\n\nIf `lab-core` is not installed, run:\n\n```bash\nclaude plugin install lab-core@{org}\n```\n\n## Required env vars\n\n{required}\n## Optional env vars\n\n{optional}",
        meta.description
    )
}

fn setup_core_command(advanced: bool) -> String {
    let (description, mode) = if advanced {
        ("Open the full Labby operator setup flow.", "full")
    } else {
        ("Open the plugin-focused Labby setup flow.", "plugin")
    };
    format!(
        "---\ndescription: {description}\nallowed-tools: Bash(labby setup:*)\n---\n\nRun the Labby setup flow:\n\n```bash\nlabby setup --mode {mode}\n```\n"
    )
}

fn install_core_command(org: &str) -> String {
    format!(
        "---\ndescription: Print the command that installs the Labby core plugin.\n---\n\nInstall the Labby core plugin, then restart Claude Code:\n\n```bash\nclaude plugin install lab-core@{org}\n```\n"
    )
}

fn install_labby_skill() -> &'static str {
    r"---
name: install-labby
description: Install the labby binary onto PATH when a lab plugin reports it missing.
---

# Install Labby

Lab plugins do not bundle the `labby` binary. If `command -v labby` fails, offer to install it:

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
```

The script downloads the latest GitHub release for this platform (sha256-verified) into `~/.local/bin/labby`, falling back to `cargo install --git https://github.com/jmagar/lab --bin labby --all-features` when no release asset exists.

After installation, run `labby setup` to start the first-run flow (config, credentials, connectivity). Never run `labby setup repair` without telling the user what it will change.

Never install other plugins, edit Claude Code config, or restart services.
"
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_readme_renders_known_service() {
        // `deploy` is one of the surviving operator-tool services post-pivot.
        // It has no required env vars; the readme should still render with the
        // standard scaffolding (description, /setup-core hint, env sections).
        let readme = service_readme("deploy", "lab");
        assert!(readme.contains("# lab-deploy"));
        assert!(readme.contains("/setup-core"));
        assert!(readme.contains("Required env vars"));
        assert!(readme.contains("Optional env vars"));
    }

    #[test]
    fn service_readme_returns_empty_for_unknown_service() {
        // Services that aren't registered (e.g. former homelab integrations
        // removed in the gateway pivot) return an empty readme rather than
        // panicking.
        let readme = service_readme("radarr", "lab");
        assert!(readme.is_empty());
    }

    #[test]
    fn service_mcp_command_resolves_labby_from_path() {
        // The plugin tree deliberately ships NO binary: service plugins invoke
        // `labby` from PATH and the install flow (scripts/install.sh →
        // `labby setup`) is explicit, not plugin-driven.
        assert_eq!(CORE_BINARY_COMMAND, "labby");
        let readme = service_readme("deploy", "lab");
        assert!(readme.contains("\"command\": \"labby\""));
        assert!(!readme.contains(".claude/plugins/lab-core/bin"));
    }
}
