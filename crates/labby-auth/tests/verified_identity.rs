use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity, VerifiedIdentityError};

#[test]
fn browser_and_bearer_authentication_share_the_same_provider_principal_link() {
    let browser = VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com",
        "google-subject-123",
    )
    .expect("browser identity should be valid");
    let bearer = VerifiedIdentity::external(
        Authenticator::OauthBearer,
        "https://accounts.google.com",
        "google-subject-123",
    )
    .expect("bearer identity should be valid");

    assert_eq!(
        browser.principal_link(),
        bearer.principal_link(),
        "authentication mechanism must not split one provider identity into multiple Principals",
    );
    assert_eq!(
        browser.principal_link(),
        &PrincipalLink::External {
            issuer: "https://accounts.google.com".to_string(),
            subject: "google-subject-123".to_string(),
        },
    );
}

#[test]
fn external_principal_links_cannot_be_derived_from_email_metadata() {
    let identity = VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com",
        "google-subject-123",
    )
    .expect("browser identity should be valid");

    assert_eq!(
        identity.principal_link(),
        &PrincipalLink::External {
            issuer: "https://accounts.google.com".to_string(),
            subject: "google-subject-123".to_string(),
        },
        "email is profile metadata and must not participate in the Principal link",
    );
}

#[test]
fn local_credentials_use_explicit_stable_ids_instead_of_a_human_provider_link() {
    let static_bearer =
        VerifiedIdentity::local_credential(Authenticator::StaticBearer, "static-bearer:primary")
            .expect("static credential identity should be valid");
    let unix_peer =
        VerifiedIdentity::local_credential(Authenticator::UnixPeer, "unix-peer:uid=1000:gid=1000")
            .expect("Unix peer identity should be valid");

    assert_eq!(
        static_bearer.principal_link(),
        &PrincipalLink::LocalCredential {
            credential_id: "static-bearer:primary".to_string(),
        },
    );
    assert_ne!(static_bearer.principal_link(), unix_peer.principal_link());
}

#[test]
fn external_identity_rejects_a_non_https_provider_issuer() {
    let result = VerifiedIdentity::external(
        Authenticator::OauthBearer,
        "accounts.google.com",
        "google-subject-123",
    );

    assert!(
        result.is_err(),
        "provider issuer must be an absolute HTTPS URL"
    );
}

#[test]
fn external_identity_records_transport_issuer_and_link_generations() {
    let browser = VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com",
        "google-subject-123",
    )
    .expect("browser identity should be valid");
    let bearer = VerifiedIdentity::external(
        Authenticator::OauthBearer,
        "https://accounts.google.com/",
        "google-subject-123",
    )
    .expect("bearer identity should be valid");

    assert_eq!(browser.transport_credential_issuer(), "browser-session");
    assert_eq!(bearer.transport_credential_issuer(), "labby-jwt");
    assert_eq!(VerifiedIdentity::VERIFICATION_SCHEMA_VERSION, 1);
    assert_eq!(VerifiedIdentity::LINK_SCHEMA_VERSION, 1);
    assert_eq!(browser.principal_link(), bearer.principal_link());
}

#[test]
fn external_identity_rejects_a_valid_but_untrusted_provider_issuer() {
    assert_eq!(
        VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://attacker.example.com",
            "subject-123",
        ),
        Err(VerifiedIdentityError::UntrustedIssuer),
    );
}

#[test]
fn identity_fingerprint_is_stable_and_does_not_disclose_identity_values() {
    let identity = VerifiedIdentity::external(
        Authenticator::OauthBearer,
        "https://accounts.google.com",
        "sensitive-provider-subject",
    )
    .expect("external identity should be valid");
    let same_identity = VerifiedIdentity::external(
        Authenticator::BrowserSession,
        "https://accounts.google.com/",
        "sensitive-provider-subject",
    )
    .expect("browser identity should be valid");

    assert_eq!(
        identity.safe_fingerprint(),
        same_identity.safe_fingerprint()
    );
    assert_eq!(identity.safe_fingerprint().len(), 12);
    assert!(!identity.safe_fingerprint().contains("sensitive"));
    assert!(!identity.safe_fingerprint().contains("google"));
}

#[test]
fn principal_link_fingerprint_supports_persisted_enterprise_issuers() {
    let persisted = PrincipalLink::External {
        issuer: "https://login.enterprise.example/oidc".to_string(),
        subject: "enterprise-subject".to_string(),
    };

    assert_eq!(persisted.safe_fingerprint().len(), 12);
    assert!(!persisted.safe_fingerprint().contains("enterprise"));
}
