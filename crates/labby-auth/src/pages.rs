//! Self-contained Aurora browser pages shared by inbound and upstream OAuth.
//!
//! Assets are built from Aurora's server-rendered OAuthDocument and canonical
//! Button, tokens and fonts. See `pages/aurora/provenance.json`; no browser
//! scripts, external asset requests or OAuth credentials are needed to render.
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

const CSS: &str = include_str!("pages/aurora/oauth.css");

/// Presentation state only; callers retain ownership of OAuth validation.
#[derive(Clone, Copy, Debug)]
pub enum OAuthPage {
    Consent,
    Success,
    Error,
    Expired,
}

impl OAuthPage {
    fn template(self) -> &'static str {
        match self {
            Self::Consent => include_str!("pages/aurora/consent.html"),
            Self::Success => include_str!("pages/aurora/success.html"),
            Self::Error => include_str!("pages/aurora/error.html"),
            Self::Expired => include_str!("pages/aurora/expired.html"),
        }
    }
}

/// Render an informational page. Text is escaped once, never interpreted as markup.
pub fn response(status: StatusCode, page: OAuthPage, title: &str, message: &str) -> Response {
    document_response(status, render(page, title, message, None))
}

/// Render a continuation link to an already validated HTTP(S) provider URL.
/// Refuse active URL schemes even if a future caller misses validation.
pub fn consent(title: &str, message: &str, provider: &url::Url) -> Response {
    if !matches!(provider.scheme(), "https" | "http")
        || !provider.username().is_empty()
        || provider.password().is_some()
    {
        return response(
            StatusCode::BAD_GATEWAY,
            OAuthPage::Error,
            "Authorization Unavailable",
            "The sign-in provider is unavailable. Return to the app and try again.",
        );
    }
    document_response(
        StatusCode::OK,
        render(OAuthPage::Consent, title, message, Some(provider.as_str())),
    )
}

fn document_response(status: StatusCode, html: String) -> Response {
    let mut response = (status, Html(html)).into_response();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

fn render(page: OAuthPage, title: &str, message: &str, action: Option<&str>) -> String {
    let mut output = String::with_capacity(CSS.len() + page.template().len() + message.len());
    let mut remainder = page.template();
    // Expand source placeholders only. Substituted text may itself contain
    // placeholder-looking strings and must never become a second template pass.
    while let Some((before, after)) = remainder.split_once("{{") {
        output.push_str(before);
        let (key, rest) = after
            .split_once("}}")
            .expect("generated template placeholder");
        match key {
            "STYLE" => output.push_str(CSS),
            "TITLE" => escape_into(title, &mut output),
            "MESSAGE" => escape_into(message, &mut output),
            "ACTION" => escape_into(action.unwrap_or_default(), &mut output),
            _ => unreachable!("unknown generated template placeholder"),
        }
        remainder = rest;
    }
    output.push_str(remainder);
    output
}

fn escape_into(value: &str, output: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_state_without_scripts_or_external_assets() {
        for page in [
            OAuthPage::Consent,
            OAuthPage::Success,
            OAuthPage::Error,
            OAuthPage::Expired,
        ] {
            let html = render(
                page,
                "Sign In",
                "Return to the app.",
                Some("https://provider.example/authorize"),
            );
            assert!(html.starts_with("<!doctype html>"));
            assert!(html.contains("aurora-page-shell"));
            assert!(html.contains("prefers-color-scheme:light"));
            assert!(html.contains(".light"));
            assert!(html.contains("font/woff2;base64,"));
            assert!(!html.contains("<script"));
            assert!(!html.contains("{{STYLE}}"));
            assert!(!html.contains("<link"));
        }
    }

    #[test]
    fn escapes_text_attributes_and_does_not_reinterpret_placeholders() {
        let html = render(
            OAuthPage::Consent,
            "<script>\"&'{{MESSAGE}}",
            "</style><img src=x onerror=alert(1)>{{STYLE}}",
            Some("https://example.test/?a=1&b=\"quoted\""),
        );
        assert!(html.contains("&lt;script&gt;&quot;&amp;&#39;{{MESSAGE}}"));
        assert!(html.contains("&lt;/style&gt;&lt;img src=x onerror=alert(1)&gt;{{STYLE}}"));
        assert!(html.contains("href=\"https://example.test/?a=1&amp;b=&quot;quoted&quot;\""));
        assert_eq!(html.matches("<style>").count(), 1);
        assert!(!html.contains("<img"));
    }

    #[test]
    fn responses_preserve_status_and_restrict_browser_capabilities() {
        let response = response(
            StatusCode::GONE,
            OAuthPage::Expired,
            "Expired",
            "Try again.",
        );
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("form-action 'none'"));
        assert!(!csp.contains("script-src"));
    }

    #[test]
    fn continuation_links_reject_active_schemes_and_embedded_credentials() {
        for url in [
            "javascript:alert(1)",
            "https://user:secret@provider.example/",
        ] {
            assert_eq!(
                consent("Sign In", "Continue.", &url::Url::parse(url).unwrap()).status(),
                StatusCode::BAD_GATEWAY
            );
        }
        assert_eq!(
            consent(
                "Sign In",
                "Continue.",
                &url::Url::parse("https://provider.example/").unwrap()
            )
            .status(),
            StatusCode::OK
        );
        // Provider configuration owns transport policy, including local test issuers.
        assert_eq!(
            consent(
                "Sign In",
                "Continue.",
                &url::Url::parse("http://127.0.0.1:8000/authorize").unwrap()
            )
            .status(),
            StatusCode::OK
        );
    }
}
