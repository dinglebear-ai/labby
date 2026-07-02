use crate::config::{OpenApiSpecConfig, SpecSource};
use crate::ssrf::validate_base_url;

fn spec(base: &str) -> OpenApiSpecConfig {
    OpenApiSpecConfig {
        label: "vendor".into(),
        spec_source: SpecSource::Url("https://api.example.com/openapi.json".parse().unwrap()),
        base_url: base.parse().unwrap(),
        allowed_operations: vec![],
        credential: None,
    }
}

#[test]
fn public_https_ok() {
    assert!(validate_base_url(&spec("https://api.example.com")).is_ok());
}

#[test]
fn rfc1918_rejected() {
    assert!(validate_base_url(&spec("https://192.168.1.10")).is_err());
}

#[test]
fn cgnat_rejected() {
    assert!(validate_base_url(&spec("https://100.64.0.1")).is_err());
}

#[test]
fn loopback_rejected() {
    assert!(validate_base_url(&spec("https://127.0.0.1")).is_err());
}

#[test]
fn plain_http_rejected() {
    assert!(validate_base_url(&spec("http://api.example.com")).is_err());
}
