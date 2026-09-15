//! Operator-owned Dev Container image-build catalog configuration.

use labby_runtime::dev_container_image_runtime::{
    ApprovedBuildExecutable, ApprovedEnvironmentSecret, ApprovedProvisionCatalog,
    ApprovedProvisionRecipe, ApprovedProvisionStep, ApprovedRuntimeNetworkProfile,
    ProvisionCatalogError, ProvisionRecipeKey,
};
use labby_runtime::dev_container_runtime::IncusProfileReference;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevContainerPreferences {
    /// Required when any recipe or network mapping is configured. Existing
    /// build snapshots pin this generation and the complete catalog digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_generation: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recipes: Vec<ProvisionRecipeConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub builder_profiles: Vec<ProfileReferenceConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runtime_network_profiles: Vec<RuntimeNetworkProfileConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub environment_secrets: Vec<EnvironmentSecretConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProvisionRecipeConfig {
    Toolchain {
        id: String,
        version: String,
        steps: Vec<ProvisionStepConfig>,
    },
    Agent {
        id: String,
        version: String,
        steps: Vec<ProvisionStepConfig>,
    },
    Package {
        registry: String,
        name: String,
        version: String,
        steps: Vec<ProvisionStepConfig>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionStepConfig {
    pub executable: ApprovedBuildExecutable,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeNetworkProfileConfig {
    pub tailnet: bool,
    pub web: bool,
    pub lan: bool,
    pub nested_docker: bool,
    pub profiles: Vec<ProfileReferenceConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReferenceConfig {
    pub name: String,
    pub content_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSecretConfig {
    pub reference: String,
    pub source_env: String,
    pub allowed_target_names: Vec<String>,
}

impl DevContainerPreferences {
    pub(super) fn validate(&self) -> Result<(), String> {
        self.catalog().map(drop).map_err(|error| error.to_string())
    }

    pub(crate) fn catalog(&self) -> Result<ApprovedProvisionCatalog, ProvisionCatalogError> {
        if self.catalog_generation.is_none()
            && self.recipes.is_empty()
            && self.builder_profiles.is_empty()
            && self.runtime_network_profiles.is_empty()
            && self.environment_secrets.is_empty()
        {
            return Ok(ApprovedProvisionCatalog::deny_all());
        }
        let generation = self
            .catalog_generation
            .clone()
            .ok_or(ProvisionCatalogError::InvalidGeneration)?;
        if self
            .runtime_network_profiles
            .iter()
            .any(|network| network.nested_docker)
        {
            return Err(ProvisionCatalogError::InvalidNetworkProfile);
        }
        ApprovedProvisionCatalog::new(
            generation,
            self.recipes.iter().cloned().map(Into::into).collect(),
            self.builder_profiles
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
            self.runtime_network_profiles
                .iter()
                .map(|network| ApprovedRuntimeNetworkProfile {
                    mask: network.mask(),
                    profiles: network.profiles.iter().cloned().map(Into::into).collect(),
                })
                .collect(),
            self.environment_secrets
                .iter()
                .map(|secret| ApprovedEnvironmentSecret {
                    reference: secret.reference.clone(),
                    source_env: secret.source_env.clone(),
                    allowed_target_names: secret.allowed_target_names.clone(),
                })
                .collect(),
        )
    }
}

impl RuntimeNetworkProfileConfig {
    #[must_use]
    fn mask(&self) -> u8 {
        u8::from(self.tailnet)
            | (u8::from(self.web) << 1)
            | (u8::from(self.lan) << 2)
            | (u8::from(self.nested_docker) << 3)
    }
}

impl From<ProfileReferenceConfig> for IncusProfileReference {
    fn from(value: ProfileReferenceConfig) -> Self {
        Self {
            name: value.name,
            content_digest: value.content_digest,
        }
    }
}

impl From<ProvisionRecipeConfig> for ApprovedProvisionRecipe {
    fn from(value: ProvisionRecipeConfig) -> Self {
        let (key, steps) = match value {
            ProvisionRecipeConfig::Toolchain { id, version, steps } => {
                (ProvisionRecipeKey::Toolchain { id, version }, steps)
            }
            ProvisionRecipeConfig::Agent { id, version, steps } => {
                (ProvisionRecipeKey::Agent { id, version }, steps)
            }
            ProvisionRecipeConfig::Package {
                registry,
                name,
                version,
                steps,
            } => (
                ProvisionRecipeKey::Package {
                    registry,
                    name,
                    version,
                },
                steps,
            ),
        };
        Self {
            key,
            steps: steps
                .into_iter()
                .map(|step| ApprovedProvisionStep {
                    executable: step.executable,
                    arguments: step.args,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> DevContainerPreferences {
        toml::from_str(
            r#"
catalog_generation = "catalog-7"

[[recipes]]
kind = "toolchain"
id = "rust"
version = "1.97.1"

[[recipes.steps]]
executable = "apt_get"
args = ["install", "-y", "rustc=1.97.1"]

[[recipes]]
kind = "package"
registry = "npm"
name = "typescript"
version = "5.9.2"

[[recipes.steps]]
executable = "npm"
args = ["install", "--global", "typescript@5.9.2"]

[[builder_profiles]]
name = "labby-builder"
content_digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[runtime_network_profiles]]
web = true

[[runtime_network_profiles.profiles]]
name = "labby-runtime-web"
content_digest = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

[[environment_secrets]]
reference = "registry-token"
source_env = "LABBY_REGISTRY_TOKEN"
allowed_target_names = ["REGISTRY_TOKEN"]
"#,
        )
        .expect("catalog config")
    }

    #[test]
    fn exact_operator_entries_resolve_to_literal_commands_and_profiles() {
        let catalog = configured().catalog().expect("approved catalog");
        assert_eq!(catalog.generation(), "catalog-7");
        assert_eq!(
            catalog
                .resolve_toolchain("rust", "1.97.1")
                .expect("toolchain")[0]
                .argv(),
            ["/usr/bin/apt-get", "install", "-y", "rustc=1.97.1"]
        );
        assert_eq!(
            catalog.resolve_network(0b0010),
            Some(vec![IncusProfileReference {
                name: "labby-runtime-web".to_owned(),
                content_digest: format!("sha256:{}", "b".repeat(64)),
            }])
        );
        assert_eq!(catalog.builder_profiles()[0].name, "labby-builder");
        assert_eq!(
            catalog.resolve_environment_secret("registry-token", "REGISTRY_TOKEN"),
            Some("LABBY_REGISTRY_TOKEN")
        );
        assert!(catalog.resolve_toolchain("rust", "nightly").is_none());
        assert!(
            catalog
                .resolve_package("npm", "typescript", "latest")
                .is_none()
        );
        assert!(catalog.resolve_network(0b0110).is_none());
    }

    #[test]
    fn duplicate_recipe_and_network_keys_are_rejected() {
        let mut config = configured();
        config.recipes.push(config.recipes[0].clone());
        assert_eq!(
            config.catalog().unwrap_err(),
            ProvisionCatalogError::DuplicateRecipe
        );

        let mut config = configured();
        config
            .runtime_network_profiles
            .push(config.runtime_network_profiles[0].clone());
        assert_eq!(
            config.catalog().unwrap_err(),
            ProvisionCatalogError::DuplicateNetworkSelection
        );
    }

    #[test]
    fn nested_docker_profiles_are_rejected_while_the_adapter_is_restricted() {
        let mut config = configured();
        config.runtime_network_profiles[0].nested_docker = true;

        assert_eq!(
            config.catalog().unwrap_err(),
            ProvisionCatalogError::InvalidNetworkProfile
        );
    }

    #[test]
    fn malformed_or_unbounded_literal_argv_is_rejected() {
        let mut config = configured();
        let ProvisionRecipeConfig::Toolchain { steps, .. } = &mut config.recipes[0] else {
            panic!("toolchain fixture")
        };
        steps[0].args[0] = "bad\nargument".to_owned();
        assert_eq!(
            config.catalog().unwrap_err(),
            ProvisionCatalogError::InvalidRecipe
        );
    }

    #[test]
    fn configured_entries_require_an_explicit_generation() {
        let mut config = configured();
        config.catalog_generation = None;
        assert_eq!(
            config.catalog().unwrap_err(),
            ProvisionCatalogError::InvalidGeneration
        );
    }

    #[test]
    fn empty_config_is_a_deny_all_catalog() {
        let catalog = DevContainerPreferences::default()
            .catalog()
            .expect("built-in catalog");
        assert_eq!(catalog.resolve_network(0), None);
        assert!(catalog.resolve_network(1).is_none());
        assert!(catalog.resolve_agent("codex", "latest").is_none());
    }

    #[test]
    fn root_config_validates_the_dev_container_catalog_at_load_time() {
        let root: crate::config::LabConfig = toml::from_str(
            r#"
[dev_containers]
catalog_generation = "catalog-1"

[[dev_containers.recipes]]
kind = "agent"
id = "codex"
version = "1"

[[dev_containers.recipes.steps]]
executable = "cargo"
args = ["install", "codex-from-operator-config"]
"#,
        )
        .expect("root config");
        root.validate().expect("validated root config");

        let invalid: crate::config::LabConfig = toml::from_str(
            r#"
[dev_containers]

[[dev_containers.recipes]]
kind = "agent"
id = "codex"
version = "1"

[[dev_containers.recipes.steps]]
executable = "cargo"
args = ["install", "codex-from-operator-config"]
"#,
        )
        .expect("structurally valid root config");
        assert!(invalid.validate().is_err());
    }
}
