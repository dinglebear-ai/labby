use crate::config::{OpenApiCredential, OpenApiSpecConfig, SpecSource};

#[test]
fn debug_never_prints_credential_value() {
    let cfg = OpenApiSpecConfig {
        label: "vendor".into(),
        spec_source: SpecSource::Url("https://api.example.com/openapi.json".parse().unwrap()),
        base_url: "https://api.example.com".parse().unwrap(),
        allowed_operations: vec!["getUser".into()],
        credential: Some(OpenApiCredential::BearerToken("super-secret-token".into())),
    };
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("super-secret-token"), "credential leaked: {dbg}");
    assert!(dbg.contains("vendor"));
}

#[test]
fn debug_never_prints_api_key_value() {
    let cred = OpenApiCredential::ApiKey {
        header: "X-API-Key".into(),
        value: "sk-live-abc123".into(),
    };
    let dbg = format!("{cred:?}");
    assert!(!dbg.contains("sk-live-abc123"), "api key leaked: {dbg}");
    // Header name is non-secret and useful — keep it visible.
    assert!(dbg.contains("X-API-Key"));
}

#[test]
fn debug_redacts_spec_url_query() {
    let src = SpecSource::Url(
        "https://api.example.com/openapi.json?token=hunter2"
            .parse()
            .unwrap(),
    );
    let dbg = format!("{src:?}");
    assert!(!dbg.contains("hunter2"), "spec url query leaked: {dbg}");
}
