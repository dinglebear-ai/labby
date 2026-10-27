//! Keep operator-facing unattended OAuth examples executable against the config contract.
use labby_runtime::gateway_config::UpstreamConfig;

#[test]
fn documented_unattended_oauth_configs_deserialize_and_validate() {
    let docs = include_str!("../../../docs/runtime/CONFIG.md");
    let section = docs
        .split_once("### Unattended upstream OAuth\n")
        .expect("unattended OAuth guide")
        .1
        .split_once("### Additional upstream permissions\n")
        .expect("end of unattended examples")
        .0;
    let mut names = Vec::new();
    for block in section.split("```toml\n").skip(1) {
        let source = block.split_once("```").expect("closed TOML fence").0;
        let value: toml::Value = toml::from_str(source).expect("valid example TOML");
        let Some(upstreams) = value.get("upstream").and_then(toml::Value::as_array) else {
            continue; // The signing-key fragment replaces a table in the complete example.
        };
        for value in upstreams {
            let config: UpstreamConfig = value.clone().try_into().expect("typed OAuth config");
            config.validate().expect("valid operator example");
            names.push(config.name);
        }
    }
    assert_eq!(names, ["automation", "enterprise"]);
}
