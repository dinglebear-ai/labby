const COMPOSE: &str = include_str!("../../../deploy/team-labby/docker-compose.yml");
const HOST_ENV: &str = include_str!("../../../deploy/team-labby/.env.example");
const TEAM_ENV: &str = include_str!("../../../deploy/team-labby/team-depot.env.example");
const LABBY_ENV: &str = include_str!("../../../deploy/team-labby/labby.env.example");
const LABBY_CONFIG: &str = include_str!("../../../deploy/team-labby/config.toml.example");
const GIT_CREDENTIALS: &str =
    include_str!("../../../deploy/team-labby/team-depot-git-credentials.json.example");
const SOURCE_BOOTSTRAP: &str = include_str!("../../../deploy/team-labby/bootstrap-team-sources.sh");
const DEPLOYMENT_README: &str = include_str!("../../../deploy/team-labby/README.md");

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
