const COMPOSE: &str = include_str!("../../../deploy/team-labby/docker-compose.yml");
const HOST_ENV: &str = include_str!("../../../deploy/team-labby/.env.example");
const TEAM_ENV: &str = include_str!("../../../deploy/team-labby/team-depot.env.example");
const LABBY_ENV: &str = include_str!("../../../deploy/team-labby/labby.env.example");
const LABBY_CONFIG: &str = include_str!("../../../deploy/team-labby/config.toml.example");
const GIT_CREDENTIALS: &str =
    include_str!("../../../deploy/team-labby/team-depot-git-credentials.json.example");
const SOURCE_BOOTSTRAP: &str = include_str!("../../../deploy/team-labby/bootstrap-team-sources.sh");
const DEPLOYMENT_README: &str = include_str!("../../../deploy/team-labby/README.md");
const RELEASE_DOCKERFILE: &str = include_str!("../../../config/Dockerfile");

fn cargo_manifest(path: &std::path::Path) -> toml::Value {
    let body = std::fs::read_to_string(path).expect("Cargo manifest must be readable");
    toml::from_str(&body).expect("Cargo manifest must contain valid TOML")
}

fn release_local_package_manifests(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let workspace = cargo_manifest(&root.join("Cargo.toml"));
    let members = workspace["workspace"]["members"]
        .as_array()
        .expect("workspace members are an array");
    let mut pending = members
        .iter()
        .map(|member| {
            root.join(member.as_str().expect("workspace member is a string"))
                .join("Cargo.toml")
        })
        .collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::new();
    let mut manifests = Vec::new();

    while let Some(manifest) = pending.pop() {
        let manifest = manifest
            .canonicalize()
            .expect("local package manifest must resolve");
        if !seen.insert(manifest.clone()) {
            continue;
        }
        let parsed = cargo_manifest(&manifest);
        for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(dependencies) = parsed.get(section).and_then(toml::Value::as_table) else {
                continue;
            };
            for dependency in dependencies.values() {
                let Some(path) = dependency
                    .as_table()
                    .and_then(|table| table.get("path"))
                    .and_then(toml::Value::as_str)
                else {
                    continue;
                };
                let candidate = manifest
                    .parent()
                    .expect("manifest has a parent")
                    .join(path)
                    .join("Cargo.toml");
                if candidate.exists() {
                    let candidate = candidate.canonicalize().expect("local dependency resolves");
                    if candidate.starts_with(root) {
                        pending.push(candidate);
                    }
                }
            }
        }
        manifests.push(manifest);
    }

    manifests.sort();
    manifests
}

#[test]
fn release_dockerfile_tracks_every_local_rust_package_needed_by_the_workspace() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let manifests = release_local_package_manifests(&root);
    assert!(
        manifests
            .iter()
            .any(|manifest| manifest.ends_with("verification/crates/verify-core/Cargo.toml")),
        "transitive local dependency audit must include verification crates"
    );
    assert!(
        RELEASE_DOCKERFILE.contains("COPY verification/Cargo.toml verification/Cargo.toml"),
        "verification crates inherit their own workspace metadata, so the cache layer must copy verification/Cargo.toml"
    );
    assert!(
        RELEASE_DOCKERFILE.contains("COPY verification/ verification/"),
        "the real verification sources must replace cache-layer stubs before the final release build"
    );
    assert!(
        RELEASE_DOCKERFILE.contains("COPY formal/ formal/"),
        "labby-model embeds formal/invariants.toml, so the final build context must include formal inputs"
    );
    assert!(
        RELEASE_DOCKERFILE.contains("RUN find crates verification -type f -exec touch {} +"),
        "real workspace and verification sources must both invalidate synthetic cache stubs"
    );

    for manifest in manifests {
        let directory = manifest.parent().expect("package manifest has a directory");
        let relative = directory
            .strip_prefix(&root)
            .expect("local package lives below workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        let parsed = cargo_manifest(&manifest);
        let package = parsed["package"]["name"]
            .as_str()
            .expect("local package declares package.name");

        assert!(
            RELEASE_DOCKERFILE.contains(&format!("COPY {relative}/Cargo.toml")),
            "release Dockerfile must copy {relative}/Cargo.toml into the dependency-cache layer"
        );
        assert!(
            RELEASE_DOCKERFILE.contains(&format!("      {relative}/src \\")),
            "release Dockerfile must create a stub source directory for {relative}"
        );
        let stub_target = if directory.join("src/lib.rs").exists() {
            Some(format!("{relative}/src/lib.rs"))
        } else if directory.join("src/main.rs").exists() {
            Some(format!("{relative}/src/main.rs"))
        } else {
            None
        };
        assert!(
            stub_target.is_some(),
            "local package {relative} has no standard lib.rs or main.rs cache target"
        );
        let stub_target = stub_target.expect("cache target existence asserted above");
        if relative.starts_with("verification/") {
            assert!(
                RELEASE_DOCKERFILE.contains(&format!(
                    "echo '//! Dependency-cache stub.' > {stub_target}"
                )),
                "verification cache stub must retain crate-level docs for {relative}: {stub_target}"
            );
        } else {
            assert!(
                RELEASE_DOCKERFILE.contains(&stub_target),
                "release Dockerfile must create a stub Cargo target for {relative}: {stub_target}"
            );
        }
        assert!(
            RELEASE_DOCKERFILE.contains(&format!("cargo clean -p {package}")),
            "release Dockerfile must clean cached local package {package} before copying real sources"
        );
    }
}

#[test]
fn team_depot_mounts_a_named_private_git_credential_map() {
    assert!(HOST_ENV.contains("TEAM_DEPOT_GIT_CREDENTIALS_FILE="));
    assert!(
        TEAM_ENV
            .contains("DEPOT_GIT_CREDENTIALS_FILE=/run/secrets/team-depot-git-credentials.json")
    );
    assert!(COMPOSE.contains(":/run/secrets/team-depot-git-credentials.json:ro"));
    assert!(GIT_CREDENTIALS.contains("\"github-private\""));
    assert!(GIT_CREDENTIALS.contains("\"host\": \"github.com\""));
}

#[test]
fn team_source_bootstrap_covers_every_required_unraid_repository() {
    for (namespace, url) in [
        ("unmarket", "https://github.com/unraid/unmarket"),
        (
            "limetech-ai-skills",
            "https://github.com/unraid/limetech-ai-skills",
        ),
        (
            "limetech-elixir-skills",
            "https://github.com/unraid/limetech-elixir-skills",
        ),
    ] {
        assert!(
            SOURCE_BOOTSTRAP.contains(namespace),
            "missing namespace {namespace}"
        );
        assert!(SOURCE_BOOTSTRAP.contains(url), "missing source {url}");
    }
    assert!(SOURCE_BOOTSTRAP.contains("depot.skills.ingest_repo"));
    assert!(SOURCE_BOOTSTRAP.contains("github-private"));
    assert!(SOURCE_BOOTSTRAP.contains("sources list"));
}

#[test]
fn systemd_team_depot_documents_secure_private_git_credential_installation() {
    assert!(
        DEPLOYMENT_README.contains("/var/lib/team-labby/team-depot/secrets/git-credentials.json")
    );
    assert!(DEPLOYMENT_README.contains("/etc/team-labby/team-depot.env"));
    assert!(DEPLOYMENT_README.contains("install -m 0640 -o root -g team-depot"));
    assert!(DEPLOYMENT_README.contains("systemctl restart team-depot"));
    assert!(DEPLOYMENT_README.contains("team-depot.service.d/30-auth-mode.conf"));
    assert!(DEPLOYMENT_README.contains("Environment=DEPOT_AUTH_MODE=oauth"));
    assert!(DEPLOYMENT_README.contains("\"authMode\": \"oauth\""));
    assert!(DEPLOYMENT_README.contains("Depot.Ingest.GitCredential.validate(\"github-private\""));
}

#[test]
fn team_deployment_binds_discover_to_loopback_host_managed_depots() {
    assert!(COMPOSE.contains("network_mode: host"));
    assert!(COMPOSE.contains("127.0.0.1:4100:4100"));
    assert!(COMPOSE.contains("127.0.0.1:4101:4100"));
    assert!(COMPOSE.contains("LABBY_MCP_HTTP_HOST: 127.0.0.1"));
    assert!(LABBY_CONFIG.contains("url = \"http://127.0.0.1:4100/mcp\""));
    assert!(LABBY_CONFIG.contains("url = \"http://127.0.0.1:4101/mcp\""));
    assert!(LABBY_CONFIG.contains("legacy_migrated = true"));
    assert!(LABBY_CONFIG.contains("id = \"team-local\""));
    assert!(LABBY_CONFIG.contains("id = \"catalog-local\""));
    assert!(LABBY_CONFIG.contains("endpoint = \"http://127.0.0.1:4100/\""));
    assert!(LABBY_CONFIG.contains("endpoint = \"http://127.0.0.1:4101/\""));
    assert!(LABBY_CONFIG.contains("bearer_token_env = \"LABBY_DEPOT_PROVIDER_TEAM_LOCAL_TOKEN\""));
    assert!(
        LABBY_CONFIG.contains("bearer_token_env = \"LABBY_DEPOT_PROVIDER_CATALOG_LOCAL_TOKEN\"")
    );
    assert!(LABBY_ENV.contains("LABBY_DEPOT_PROVIDER_TEAM_LOCAL_TOKEN="));
    assert!(LABBY_ENV.contains("LABBY_DEPOT_PROVIDER_CATALOG_LOCAL_TOKEN="));
    assert!(LABBY_ENV.contains("LABBY_DEPOT_URL=http://127.0.0.1:4100"));
}

#[test]
fn team_deployment_binds_depot_mutations_to_the_publish_route_project() {
    assert!(LABBY_CONFIG.contains("name = \"team-depot-publish\""));
    assert!(LABBY_CONFIG.contains("public_path = \"/mcp/team-depot\""));
    assert!(LABBY_CONFIG.contains("[depot.publish]"));
    assert!(LABBY_CONFIG.contains("route_id = \"team-depot-publish\""));
    assert_eq!(
        LABBY_CONFIG
            .matches("project_id = \"replace-with-bound-project-id\"")
            .count(),
        4,
        "Linear target, Team Depot publish target, Depot publish binding, and Discover read binding must share the project placeholder"
    );
}

#[test]
fn lime_team_deployment_admits_the_verified_company_domain() {
    assert!(LABBY_ENV.contains("LABBY_AUTH_ALLOWED_EMAIL_DOMAINS=lime-technology.com"));
}
