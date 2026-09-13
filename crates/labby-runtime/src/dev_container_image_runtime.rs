//! Surface-neutral effects for immutable Dev Container image builds.
//!
//! Every effect method submits at most one external effect and returns its
//! operation identifier without waiting. Product code must persist authorized
//! intent before submission, then attach the returned identifier before calling
//! [`ContainerImageRuntime::wait`]. A restart can therefore distinguish a
//! prepared effect with an uncertain submission from an observed engine
//! operation and reconcile either state without repeating it blindly.

use std::{collections::BTreeMap, future::Future, pin::Pin};

use labby_primitives::dev_container::ImageDigest;
use sha2::{Digest as _, Sha256};

use crate::dev_container_runtime::IncusProfileReference;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageBuildHandle {
    pub build_id: String,
    pub lifecycle_nonce: String,
    pub builder_instance_name: String,
}

impl ImageBuildHandle {
    #[must_use]
    pub fn deterministic_name(build_id: &str, lifecycle_nonce: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(build_id.as_bytes());
        hasher.update([0]);
        hasher.update(lifecycle_nonce.as_bytes());
        let digest = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("labby-build-{}", &digest[..24])
    }

    #[must_use]
    pub fn has_valid_name(&self) -> bool {
        self.builder_instance_name
            == Self::deterministic_name(&self.build_id, &self.lifecycle_nonce)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageBuildRequest {
    pub handle: ImageBuildHandle,
    pub base_image: ImageDigest,
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    /// Exact Incus profiles selected for all builders by the operator catalog.
    /// They are independent of the requested final-instance network mask.
    pub profiles: Vec<IncusProfileReference>,
}

/// A command selected from the trusted product catalog. The executable is
/// private and cannot be supplied by callers; arguments are validated by the
/// catalog before this value can be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionCommand {
    executable: ApprovedBuildExecutable,
    arguments: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovedBuildExecutable {
    AptGet,
    Npm,
    Uv,
    Cargo,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProvisionRecipeKey {
    Toolchain {
        id: String,
        version: String,
    },
    Agent {
        id: String,
        version: String,
    },
    Package {
        registry: String,
        name: String,
        version: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ApprovedProvisionStep {
    pub executable: ApprovedBuildExecutable,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ApprovedProvisionRecipe {
    pub key: ProvisionRecipeKey,
    pub steps: Vec<ApprovedProvisionStep>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ApprovedRuntimeNetworkProfile {
    pub mask: u8,
    pub profiles: Vec<IncusProfileReference>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ApprovedEnvironmentSecret {
    pub reference: String,
    pub source_env: String,
    pub allowed_target_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, thiserror::Error, PartialEq)]
pub enum ProvisionCatalogError {
    #[error("the provisioning catalog generation is invalid")]
    InvalidGeneration,
    #[error("the provisioning catalog is too large")]
    TooLarge,
    #[error("a provisioning recipe key is invalid")]
    InvalidRecipeKey,
    #[error("a provisioning recipe is invalid")]
    InvalidRecipe,
    #[error("a provisioning recipe key is duplicated")]
    DuplicateRecipe,
    #[error("a build network profile is invalid")]
    InvalidNetworkProfile,
    #[error("a build network selection is duplicated")]
    DuplicateNetworkSelection,
    #[error("an Incus profile reference is invalid")]
    InvalidProfileReference,
    #[error("an environment secret binding is invalid")]
    InvalidEnvironmentSecret,
    #[error("an environment secret reference is duplicated")]
    DuplicateEnvironmentSecret,
}

impl ProvisionCommand {
    #[must_use]
    pub fn argv(&self) -> Vec<&str> {
        let executable = match self.executable {
            ApprovedBuildExecutable::AptGet => "/usr/bin/apt-get",
            ApprovedBuildExecutable::Npm => "/usr/bin/npm",
            ApprovedBuildExecutable::Uv => "/usr/local/bin/uv",
            ApprovedBuildExecutable::Cargo => "/root/.cargo/bin/cargo",
        };
        std::iter::once(executable)
            .chain(self.arguments.iter().map(String::as_str))
            .collect()
    }
}

/// A validated, immutable operator provisioning catalog. Product dispatch may
/// resolve exact request keys through it, but cannot construct a command from
/// a caller-supplied executable or argument.
#[derive(Clone, Debug)]
pub struct ApprovedProvisionCatalog {
    generation: String,
    digest: String,
    recipes: BTreeMap<ProvisionRecipeKey, Vec<ProvisionCommand>>,
    builder_profiles: Vec<IncusProfileReference>,
    networks: BTreeMap<u8, Vec<IncusProfileReference>>,
    environment_secrets: BTreeMap<String, ApprovedEnvironmentSecret>,
}

impl Default for ApprovedProvisionCatalog {
    fn default() -> Self {
        Self::deny_all()
    }
}

impl ApprovedProvisionCatalog {
    pub fn new(
        generation: String,
        mut recipes: Vec<ApprovedProvisionRecipe>,
        builder_profiles: Vec<IncusProfileReference>,
        mut networks: Vec<ApprovedRuntimeNetworkProfile>,
        mut environment_secrets: Vec<ApprovedEnvironmentSecret>,
    ) -> Result<Self, ProvisionCatalogError> {
        if generation.is_empty()
            || generation.len() > 256
            || generation != generation.trim()
            || generation.chars().any(char::is_control)
        {
            return Err(ProvisionCatalogError::InvalidGeneration);
        }
        if recipes.len() > 256
            || builder_profiles.len() > 8
            || networks.len() > 16
            || environment_secrets.len() > 256
        {
            return Err(ProvisionCatalogError::TooLarge);
        }
        recipes.sort_by(|left, right| left.key.cmp(&right.key));
        networks.sort_by_key(|network| network.mask);
        environment_secrets.sort_by(|left, right| left.reference.cmp(&right.reference));
        for secret in &mut environment_secrets {
            secret.allowed_target_names.sort();
        }

        let canonical = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "generation": &generation,
            "recipes": &recipes,
            "builder_profiles": &builder_profiles,
            "networks": &networks,
            "environment_secrets": &environment_secrets,
        }))
        .map_err(|_| ProvisionCatalogError::TooLarge)?;
        if canonical.len() > 1024 * 1024 {
            return Err(ProvisionCatalogError::TooLarge);
        }
        let digest = format!("sha256:{}", hex_digest(&Sha256::digest(&canonical)));

        let mut resolved_recipes = BTreeMap::new();
        for recipe in recipes {
            validate_recipe_key(&recipe.key)?;
            if recipe.steps.is_empty() || recipe.steps.len() > 16 {
                return Err(ProvisionCatalogError::InvalidRecipe);
            }
            let mut commands = Vec::with_capacity(recipe.steps.len());
            for step in recipe.steps {
                if step.arguments.len() > 64
                    || step.arguments.iter().any(|argument| {
                        argument.is_empty()
                            || argument.len() > 4096
                            || argument.chars().any(char::is_control)
                    })
                {
                    return Err(ProvisionCatalogError::InvalidRecipe);
                }
                commands.push(Self::command(step.executable, step.arguments));
            }
            if resolved_recipes.insert(recipe.key, commands).is_some() {
                return Err(ProvisionCatalogError::DuplicateRecipe);
            }
        }

        validate_profile_set(&builder_profiles)?;
        if !networks.is_empty() && builder_profiles.is_empty() {
            return Err(ProvisionCatalogError::InvalidProfileReference);
        }
        let mut resolved_networks = BTreeMap::new();
        for network in networks {
            if network.mask > 0b1111
                || network.mask & 0b1000 != 0
                || network.profiles.len() > 8
                || network.profiles.is_empty()
            {
                return Err(ProvisionCatalogError::InvalidNetworkProfile);
            }
            validate_profile_set(&network.profiles)?;
            if resolved_networks
                .insert(network.mask, network.profiles)
                .is_some()
            {
                return Err(ProvisionCatalogError::DuplicateNetworkSelection);
            }
        }

        let mut resolved_environment_secrets = BTreeMap::new();
        for secret in environment_secrets {
            if !valid_catalog_text(&secret.reference, 256)
                || !valid_environment_name(&secret.source_env)
                || secret.allowed_target_names.is_empty()
                || secret.allowed_target_names.len() > 32
                || secret
                    .allowed_target_names
                    .iter()
                    .any(|name| !valid_environment_name(name))
            {
                return Err(ProvisionCatalogError::InvalidEnvironmentSecret);
            }
            let mut unique_targets = secret.allowed_target_names.clone();
            unique_targets.sort();
            unique_targets.dedup();
            if unique_targets.len() != secret.allowed_target_names.len()
                || resolved_environment_secrets
                    .insert(secret.reference.clone(), secret)
                    .is_some()
            {
                return Err(ProvisionCatalogError::DuplicateEnvironmentSecret);
            }
        }

        Ok(Self {
            generation,
            digest,
            recipes: resolved_recipes,
            builder_profiles,
            networks: resolved_networks,
            environment_secrets: resolved_environment_secrets,
        })
    }

    #[must_use]
    pub fn deny_all() -> Self {
        Self::new(
            "builtin-deny-all-v1".to_owned(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("the built-in deny-all catalog is valid")
    }

    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn resolve_toolchain(&self, id: &str, version: &str) -> Option<Vec<ProvisionCommand>> {
        self.resolve(&ProvisionRecipeKey::Toolchain {
            id: id.to_owned(),
            version: version.to_owned(),
        })
    }

    #[must_use]
    pub fn resolve_agent(&self, id: &str, version: &str) -> Option<Vec<ProvisionCommand>> {
        self.resolve(&ProvisionRecipeKey::Agent {
            id: id.to_owned(),
            version: version.to_owned(),
        })
    }

    #[must_use]
    pub fn resolve_package(
        &self,
        registry: &str,
        name: &str,
        version: &str,
    ) -> Option<Vec<ProvisionCommand>> {
        self.resolve(&ProvisionRecipeKey::Package {
            registry: registry.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
        })
    }

    #[must_use]
    pub fn builder_profiles(&self) -> Vec<IncusProfileReference> {
        self.builder_profiles.clone()
    }

    #[must_use]
    pub fn resolve_network(&self, mask: u8) -> Option<Vec<IncusProfileReference>> {
        self.networks.get(&mask).cloned()
    }

    #[must_use]
    pub fn resolve_environment_secret(&self, reference: &str, target_name: &str) -> Option<&str> {
        self.environment_secrets.get(reference).and_then(|secret| {
            secret
                .allowed_target_names
                .iter()
                .any(|name| name == target_name)
                .then_some(secret.source_env.as_str())
        })
    }

    pub fn environment_source_names(&self) -> impl Iterator<Item = &str> {
        self.environment_secrets
            .values()
            .map(|secret| secret.source_env.as_str())
    }

    fn resolve(&self, key: &ProvisionRecipeKey) -> Option<Vec<ProvisionCommand>> {
        self.recipes.get(key).cloned()
    }

    fn command(executable: ApprovedBuildExecutable, arguments: Vec<String>) -> ProvisionCommand {
        ProvisionCommand {
            executable,
            arguments,
        }
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn profile(name: &str, digest_byte: char) -> IncusProfileReference {
        IncusProfileReference {
            name: name.to_owned(),
            content_digest: format!("sha256:{}", digest_byte.to_string().repeat(64)),
        }
    }

    #[test]
    fn builder_profile_order_is_preserved_in_resolution_and_digest() {
        let base = profile("labby-builder-base", 'a');
        let override_profile = profile("labby-builder-override", 'b');
        let ordered = ApprovedProvisionCatalog::new(
            "catalog-1".to_owned(),
            Vec::new(),
            vec![base.clone(), override_profile.clone()],
            Vec::new(),
            Vec::new(),
        )
        .expect("ordered catalog");
        let reversed = ApprovedProvisionCatalog::new(
            "catalog-1".to_owned(),
            Vec::new(),
            vec![override_profile.clone(), base.clone()],
            Vec::new(),
            Vec::new(),
        )
        .expect("reversed catalog");

        assert_eq!(ordered.builder_profiles(), vec![base, override_profile]);
        assert_ne!(ordered.digest(), reversed.digest());
    }

    #[test]
    fn restricted_runtime_catalog_rejects_nested_docker_network_masks() {
        let error = ApprovedProvisionCatalog::new(
            "catalog-1".to_owned(),
            Vec::new(),
            vec![profile("labby-builder", 'a')],
            vec![ApprovedRuntimeNetworkProfile {
                mask: 0b1000,
                profiles: vec![profile("labby-runtime-nested", 'b')],
            }],
            Vec::new(),
        )
        .unwrap_err();

        assert_eq!(error, ProvisionCatalogError::InvalidNetworkProfile);
    }
}

fn validate_recipe_key(key: &ProvisionRecipeKey) -> Result<(), ProvisionCatalogError> {
    let valid = match key {
        ProvisionRecipeKey::Toolchain { id, version }
        | ProvisionRecipeKey::Agent { id, version } => {
            valid_catalog_text(id, 128) && valid_catalog_text(version, 64)
        }
        ProvisionRecipeKey::Package {
            registry,
            name,
            version,
        } => {
            valid_catalog_text(registry, 16)
                && valid_catalog_text(name, 128)
                && valid_catalog_text(version, 64)
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ProvisionCatalogError::InvalidRecipeKey)
    }
}

fn valid_catalog_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_profile_name(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn validate_profile_set(profiles: &[IncusProfileReference]) -> Result<(), ProvisionCatalogError> {
    let unique_names = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if unique_names.len() != profiles.len()
        || profiles.iter().any(|profile| {
            !valid_profile_name(&profile.name)
                || profile.content_digest.len() != 71
                || !profile.content_digest.starts_with("sha256:")
                || profile.content_digest[7..].chars().any(|character| {
                    !character.is_ascii_hexdigit() || character.is_ascii_uppercase()
                })
        })
    {
        return Err(ProvisionCatalogError::InvalidProfileReference);
    }
    Ok(())
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphanumeric() && (index > 0 || !byte.is_ascii_digit())
        })
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageBuildEffect {
    Launch,
    Provision,
    Stop,
    Publish,
    Cleanup,
    Discard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageBuildOperation {
    pub effect: ImageBuildEffect,
    pub operation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageBuildOperationResult {
    pub image_digest: Option<ImageDigest>,
}

pub trait ContainerImageRuntime: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn launch<'a>(
        &'a self,
        request: ImageBuildRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn provision<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
        command: &'a ProvisionCommand,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn stop<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn publish<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn cleanup<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn published_image<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ImageDigest>, Self::Error>> + Send + 'a>>;

    fn discard_image<'a>(
        &'a self,
        handle: &'a ImageBuildHandle,
        image: &'a ImageDigest,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>;

    fn wait<'a>(
        &'a self,
        operation: &'a ImageBuildOperation,
    ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperationResult, Self::Error>> + Send + 'a>>;
}
