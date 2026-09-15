const COMPOSE: &str = include_str!("../../../deploy/team-labby/docker-compose.yml");
const HOST_ENV: &str = include_str!("../../../deploy/team-labby/.env.example");
const TEAM_ENV: &str = include_str!("../../../deploy/team-labby/team-depot.env.example");
const LABBY_ENV: &str = include_str!("../../../deploy/team-labby/labby.env.example");
const GIT_CREDENTIALS: &str =
    include_str!("../../../deploy/team-labby/team-depot-git-credentials.json.example");
const SOURCE_BOOTSTRAP: &str = include_str!("../../../deploy/team-labby/bootstrap-team-sources.sh");

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
fn lime_team_deployment_admits_the_verified_company_domain() {
    assert!(LABBY_ENV.contains("LABBY_AUTH_ALLOWED_EMAIL_DOMAINS=lime-technology.com"));
}
