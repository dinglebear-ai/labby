use base64::Engine as _;
use labby_primitives::product_credential::{
    PRODUCT_CREDENTIAL_PREFIX, ProductCredential, ProductCredentialParseError,
    ProductCredentialSelection, select_product_credential,
};

fn token(id: &str, secret: [u8; 32]) -> String {
    format!(
        "{PRODUCT_CREDENTIAL_PREFIX}{id}_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    )
}

#[test]
fn canonical_credentials_bind_id_and_secret_without_fallback() {
    let wire = token("credential-id", [0xA5; 32]);
    let parsed = ProductCredential::parse(&wire).unwrap();
    assert_eq!(parsed.credential_id(), "credential-id");
    assert_eq!(parsed.secret(), &[0xA5; 32]);
    assert!(matches!(
        select_product_credential(&wire),
        ProductCredentialSelection::Parsed(_)
    ));
    assert!(matches!(
        select_product_credential("ordinary.jwt.token"),
        ProductCredentialSelection::NotProductCredential
    ));
}

#[test]
fn malformed_tampered_and_oversized_product_prefixes_are_terminal() {
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32]);
    for wire in [
        "lby_pc_v1_bad".to_owned(),
        format!("{PRODUCT_CREDENTIAL_PREFIX}bad_id_{secret}"),
        format!("{PRODUCT_CREDENTIAL_PREFIX}credential_{secret}="),
        format!("{PRODUCT_CREDENTIAL_PREFIX}{}_{}", "x".repeat(65), secret),
    ] {
        assert!(matches!(
            select_product_credential(&wire),
            ProductCredentialSelection::Malformed(_)
        ));
    }
    assert!(matches!(
        ProductCredential::parse("lby_pc_v1_bad"),
        Err(ProductCredentialParseError::Malformed)
    ));
}

#[test]
fn parser_errors_and_type_surface_never_render_the_secret() {
    let canary = "SUPER_SECRET_CREDENTIAL_CANARY";
    let parsed =
        ProductCredential::parse(&format!("{PRODUCT_CREDENTIAL_PREFIX}credential-{canary}"));
    assert!(parsed.is_err(), "invalid canary-bearing credential parsed");
    let error = parsed.err().unwrap().to_string();
    assert!(!error.contains(canary));
    assert_eq!(error, "product credential has an invalid canonical form");
}
