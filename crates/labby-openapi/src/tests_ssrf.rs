use crate::config::{OpenApiSpecConfig, SpecSource};
use crate::ssrf::{validate_base_url, validate_spec_url};

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

#[test]
fn spec_url_public_https_ok() {
    let url = "https://api.example.com/openapi.json".parse().unwrap();
    assert!(validate_spec_url("vendor", &url).is_ok());
}

#[test]
fn spec_url_private_and_non_https_rejected() {
    // The remote spec document URL is guarded like base_url — before any fetch.
    let rfc1918 = "https://10.0.0.5/openapi.json".parse().unwrap();
    assert!(validate_spec_url("vendor", &rfc1918).is_err());
    let http = "http://api.example.com/openapi.json".parse().unwrap();
    assert!(validate_spec_url("vendor", &http).is_err());
    let loopback = "https://127.0.0.1/openapi.json".parse().unwrap();
    assert!(validate_spec_url("vendor", &loopback).is_err());
}
