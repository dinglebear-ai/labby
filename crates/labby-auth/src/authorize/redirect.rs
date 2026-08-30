//! Redirect URI classification and configured-pattern matching.

fn is_loopback_redirect(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

fn is_native_app_scheme_redirect(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    !matches!(
        url.scheme(),
        "http" | "https" | "javascript" | "data" | "vbscript" | "file"
    )
}

pub(crate) fn is_allowed_redirect_uri(value: &str, patterns: &[String]) -> bool {
    if is_loopback_redirect(value) || is_native_app_scheme_redirect(value) {
        return true;
    }
    let Ok(candidate) = reqwest::Url::parse(value) else {
        return false;
    };
    patterns
        .iter()
        .any(|pattern| redirect_pattern_matches(pattern, &candidate))
}

pub(super) fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }
    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let non_empty_parts: Vec<&str> = parts.into_iter().filter(|part| !part.is_empty()).collect();
    if non_empty_parts.is_empty() {
        return true;
    }
    let mut cursor = 0usize;
    for (index, part) in non_empty_parts.iter().enumerate() {
        if index == 0 && anchored_start {
            if !value[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
            continue;
        }
        match value[cursor..].find(part) {
            Some(found) => cursor += found + part.len(),
            None => return false,
        }
    }
    if anchored_end && let Some(last) = non_empty_parts.last() {
        return value.ends_with(last);
    }
    true
}

fn redirect_pattern_matches(pattern: &str, candidate: &reqwest::Url) -> bool {
    if pattern == "https://*" {
        return candidate.scheme() == "https" && candidate.host_str().is_some();
    }
    let Ok(pattern_url) = reqwest::Url::parse(pattern) else {
        return false;
    };
    if pattern_url.scheme() != candidate.scheme() {
        return false;
    }
    if pattern_url.host_str().is_none() || candidate.host_str().is_none() {
        return wildcard_matches(pattern, candidate.as_str());
    }
    if pattern_url.port_or_known_default() != candidate.port_or_known_default() {
        return false;
    }
    let (Some(pattern_host), Some(candidate_host)) = (pattern_url.host_str(), candidate.host_str())
    else {
        return false;
    };
    if !host_pattern_matches(pattern_host, candidate_host)
        || !wildcard_matches(pattern_url.path(), candidate.path())
    {
        return false;
    }
    match (pattern_url.query(), candidate.query()) {
        (Some(pattern_query), Some(candidate_query)) => {
            wildcard_matches(pattern_query, candidate_query)
        }
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn host_pattern_matches(pattern_host: &str, candidate_host: &str) -> bool {
    let pattern_labels = pattern_host.split('.').collect::<Vec<_>>();
    let candidate_labels = candidate_host.split('.').collect::<Vec<_>>();
    pattern_labels.len() == candidate_labels.len()
        && pattern_labels
            .iter()
            .zip(candidate_labels.iter())
            .all(|(pattern, candidate)| {
                *pattern == "*"
                    || (!pattern.contains('*') && pattern.eq_ignore_ascii_case(candidate))
            })
}
