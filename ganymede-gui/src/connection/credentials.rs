//! Credential keys include the sanitized deployment prefix and an app namespace.
//! The platform keychain owns persistence; history contains only an opt-in flag.
pub(super) fn key(address: &str) -> String {
    format!("ganymede:jupyter:{address}")
}

/// Call only after parse_connection has validated the URL and token.
pub(super) fn token(address: &str, explicit: &str) -> Option<String> {
    if !explicit.trim().is_empty() {
        return Some(explicit.trim().to_owned());
    }
    reqwest::Url::parse(address.trim())
        .ok()?
        .query_pairs()
        .filter(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned())
        .last()
        .filter(|value| !value.is_empty())
}

// GPUI's macOS implementation exposes Security.framework's status in its error.
// Removing an already absent item is successful too (errSecItemNotFound).
pub(super) fn deleted(result: &anyhow::Result<()>) -> bool {
    result.is_ok()
        || (cfg!(target_os = "macos")
            && result
                .as_ref()
                .err()
                .is_some_and(|error| error.to_string() == "delete password failed: -25300"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::server::parse_connection;

    #[test]
    fn credential_identity_normalizes_urls_but_separates_deployments() {
        let (a, _) =
            parse_connection("https://EXAMPLE.test:443/user/alice/lab?token=secret", "").unwrap();
        let (b, _) = parse_connection("https://example.test/user/alice/", "").unwrap();
        assert_eq!(key(&a), key(&b));
        assert_ne!(key(&a), key("https://example.test/user/bob/"));
        assert_ne!(key(&a), key("http://example.test/user/alice/"));
        assert!(!key(&a).contains("secret"));
        assert!(key(&a).starts_with("ganymede:jupyter:"));
    }

    #[test]
    fn forgetting_an_absent_token_is_safe_but_access_failures_are_not_success() {
        assert!(deleted(&Ok(())));
        assert!(!deleted(&Err(anyhow::anyhow!(
            "delete password failed: -25293"
        ))));
        if cfg!(target_os = "macos") {
            assert!(deleted(&Err(anyhow::anyhow!(
                "delete password failed: -25300"
            ))));
        }
    }

    #[test]
    fn explicit_token_overrides_pasted_token_and_empty_is_not_saved() {
        assert_eq!(
            token("https://host/lab?token=a%2Bb", ""),
            Some("a+b".into())
        );
        assert_eq!(
            token("https://host/?token=old", " new "),
            Some("new".into())
        );
        assert_eq!(token("https://host/", ""), None);
        assert_eq!(token("https://host/?token=", ""), None);
    }
}
