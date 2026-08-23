use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};

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
