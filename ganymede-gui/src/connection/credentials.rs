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

/// The name used in help and errors; backend errors can contain secrets and
/// should never be shown directly.
pub(super) fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else {
        "system keyring"
    }
}

pub(super) fn help() -> String {
    format!(
        "Remember token saves it in the {}. Turn it off to connect without saving. Use Forget token to remove an existing saved token.",
        store_name()
    )
}

pub(super) fn save_error() -> String {
    format!(
        "The server connection worked, but the token could not be saved to the {}. Unlock it and retry, or turn off Remember token and connect again.",
        store_name()
    )
}

pub(super) fn forget_error() -> String {
    format!(
        "Could not remove the saved token from the {}. Make sure it is available and unlocked, then try Forget token again. The saved-token record has been kept.",
        store_name()
    )
}

/// Never touch the credential store unless the user requested persistence and
/// the caller has already validated the server connection.
pub(super) async fn save<F, Fut>(
    request: Option<(String, String)>,
    write: F,
) -> Result<bool, String>
where
    F: FnOnce(String, String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let Some((address, token)) = request else {
        return Ok(false);
    };
    write(key(&address), token)
        .await
        .map_err(|_| save_error())?;
    Ok(true)
}

pub(super) type StoredToken = anyhow::Result<Option<(String, Vec<u8>)>>;

pub(super) fn restored(result: StoredToken) -> Result<String, String> {
    match result {
        Ok(Some((_, bytes))) => String::from_utf8(bytes)
            .ok().filter(|token| !token.trim().is_empty())
            .ok_or_else(|| format!("The saved token is invalid. Enter it again to connect without saving, or enable Remember token to replace it in the {}.", store_name())),
        Ok(None) => Err("The saved token was not found. Enter it again to connect without saving, or enable Remember token to save a new copy.".into()),
        Err(_) => Err(format!("Could not read the saved token from the {}. Enter it again to connect without saving, or unlock the credential store and retry the recent connection.", store_name())),
    }
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

    #[tokio::test]
    async fn connecting_without_remembering_never_accesses_the_store() {
        let saved = save(None, |_, _| async {
            panic!("The keyring must not be accessed without a save request");
        })
        .await
        .unwrap();
        assert!(!saved);
    }

    #[tokio::test]
    async fn requested_tokens_use_namespaced_keys_and_only_success_counts_as_saved() {
        let saved = save(
            Some(("https://host/prefix/".into(), "test-token".into())),
            |key, token| async move {
                assert_eq!(key, "ganymede:jupyter:https://host/prefix/");
                assert_eq!(token, "test-token");
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(saved);

        let error = save(
            Some(("https://host/".into(), "test-token".into())),
            |_, _| async { Err(anyhow::anyhow!("backend failure containing test-token")) },
        )
        .await
        .unwrap_err();
        assert!(error.contains("turn off Remember token"));
        assert!(error.contains(store_name()));
        assert!(!error.contains("test-token"));
    }

    #[test]
    fn restore_distinguishes_missing_invalid_and_unavailable_without_exposing_secrets() {
        assert_eq!(
            restored(Ok(Some(("token".into(), b"saved-token".to_vec())))).unwrap(),
            "saved-token"
        );
        assert!(restored(Ok(None)).unwrap_err().contains("not found"));
        for bytes in [vec![0xff], Vec::new(), b"  ".to_vec()] {
            assert!(
                restored(Ok(Some(("token".into(), bytes))))
                    .unwrap_err()
                    .contains("invalid")
            );
        }
        let error = restored(Err(anyhow::anyhow!("locked: secret-token"))).unwrap_err();
        assert!(error.contains("without saving"));
        assert!(error.contains(store_name()));
        assert!(!error.contains("secret-token"));
    }
}
