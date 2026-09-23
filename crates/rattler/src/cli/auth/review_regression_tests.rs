use super::*;
use rattler_networking::authentication_storage::{
    StorageBackend,
    authentication::{OAuthOidcMetadata, is_oidc_scope},
    backends::memory::{MemoryStorage, MemoryStorageError},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn config(scopes: &[&str]) -> oauth::OAuthConfig {
    oauth_config_for_host("prefix.dev", scopes).unwrap()
}

fn grant(config: &oauth::OAuthConfig) -> Authentication {
    Authentication::OAuth {
        access_token: "test-opaque".into(),
        refresh_token: Some("test-refresh".into()),
        expires_at: None,
        token_endpoint: "https://prefix.dev/token".into(),
        revocation_endpoint: None,
        client_id: config.client_id.clone(),
        issuer_url: Some(config.issuer_url.clone()),
        scopes: Some(
            config
                .scopes
                .iter()
                .filter(|s| !is_oidc_scope(s))
                .cloned()
                .collect(),
        ),
        oidc: Some(OAuthOidcMetadata {
            requested_scopes: config
                .scopes
                .iter()
                .filter(|s| is_oidc_scope(s))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            id_token_verified: true,
        }),
    }
}

#[derive(Debug, Default)]
struct FaultyBackend {
    inner: MemoryStorage,
    fail_reads: AtomicBool,
    fail_writes: AtomicBool,
    writes: AtomicUsize,
}
impl StorageBackend for FaultyBackend {
    fn name(&self) -> String {
        "test-backend".into()
    }
    fn get(&self, key: &str) -> Result<Option<Authentication>, AuthenticationStorageError> {
        if self.fail_reads.load(Ordering::SeqCst) {
            return Err(MemoryStorageError::LockError.into());
        }
        self.inner.get(key)
    }
    fn store(&self, key: &str, auth: &Authentication) -> Result<(), AuthenticationStorageError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(MemoryStorageError::LockError.into());
        }
        self.inner.store(key, auth)
    }
    fn delete(&self, key: &str) -> Result<(), AuthenticationStorageError> {
        self.inner.delete(key)
    }
}

fn storage() -> (AuthenticationStorage, Arc<FaultyBackend>) {
    let backend = Arc::new(FaultyBackend::default());
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(backend.clone());
    (storage, backend)
}

#[tokio::test]
async fn unreadable_grants_abort_even_with_negative_or_positive_best_effort_cache() {
    for cached_positive in [false, true] {
        let (storage, backend) = storage();
        let old = grant(&config(&["previous:permission"]));
        backend.inner.store("prefix.dev", &old).unwrap();
        backend.fail_reads.store(!cached_positive, Ordering::SeqCst);
        assert_eq!(
            storage.get("prefix.dev").unwrap().is_some(),
            cached_positive
        );
        backend.fail_reads.store(true, Ordering::SeqCst);
        for replace in [false, true] {
            let result = login_oauth_and_store(
                "prefix.dev",
                &storage,
                config(&["new:permission"]),
                replace,
                |_| async { panic!("read failure must abort before authorization") },
            )
            .await;
            assert!(result.is_err());
            assert_eq!(backend.writes.load(Ordering::SeqCst), 0);
            assert_eq!(backend.inner.get("prefix.dev").unwrap(), Some(old.clone()));
        }
    }
}

#[test]
fn strict_lookup_does_not_fall_back_past_an_unreadable_backend() {
    let (mut storage, backend) = storage();
    backend.fail_reads.store(true, Ordering::SeqCst);
    let fallback = Arc::new(MemoryStorage::new());
    fallback
        .store("prefix.dev", &grant(&config(&["fallback:permission"])))
        .unwrap();
    storage.add_backend(fallback);
    // Existing middleware behavior stays best-effort.
    assert!(storage.get("prefix.dev").unwrap().is_some());
    // Grant updates must not mistake the shadowed fallback for the full grant.
    assert!(
        storage
            .get_by_url_with_host_strict(&Url::parse("https://prefix.dev").unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn failed_persistence_preserves_disk_and_cached_credentials() {
    for replace in [false, true] {
        let (storage, backend) = storage();
        let old = grant(&config(&["previous:permission"]));
        storage.store("prefix.dev", &old).unwrap();
        backend.fail_writes.store(true, Ordering::SeqCst);
        let result = login_oauth_and_store(
            "prefix.dev",
            &storage,
            config(&["new:permission"]),
            replace,
            |config| async move { Ok(grant(&config)) },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(storage.get("prefix.dev").unwrap(), Some(old.clone()));
        assert_eq!(backend.inner.get("prefix.dev").unwrap(), Some(old));
    }
}

#[tokio::test]
async fn resource_only_token_scopes_allow_login_and_noninteractive_reuse() {
    let (storage, _) = storage();
    login_oauth_and_store(
        "prefix.dev",
        &storage,
        config(&["custom:read"]),
        false,
        |config| async move { Ok(grant(&config)) },
    )
    .await
    .unwrap();
    let auth = storage.get("prefix.dev").unwrap().unwrap();
    assert_eq!(auth.oauth_scopes().unwrap(), ["custom:read"]);
    let cached = oauth::ensure_oauth_scopes(
        config(&["custom:read"]),
        Some(&auth),
        oauth::OAuthInteraction::Deny,
    )
    .await
    .unwrap();
    assert_eq!(cached, auth);
    assert!(auth.oauth_oidc_metadata().unwrap().id_token_verified);
}

#[tokio::test]
async fn oidc_request_scopes_survive_extension_but_missing_resource_permissions_fail() {
    for partial in [false, true] {
        let (storage, _) = storage();
        let old = grant(&config(&["email", "previous:permission"]));
        storage.store("prefix.dev", &old).unwrap();
        let result = login_oauth_and_store(
            "prefix.dev",
            &storage,
            config(&["new:permission"]),
            false,
            |mut config| async move {
                assert!(config.scopes.contains("email"));
                assert!(config.scopes.contains("previous:permission"));
                if partial {
                    config.scopes.remove("previous:permission");
                }
                Ok(grant(&config))
            },
        )
        .await;
        if partial {
            assert!(matches!(
                result,
                Err(AuthenticationCLIError::OAuthScopes(
                    oauth::EnsureScopesError::IncompleteGrant
                ))
            ));
            assert_eq!(storage.get("prefix.dev").unwrap(), Some(old));
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn replacement_is_an_explicit_oauth_only_cli_option() {
    assert!(LoginArgs::try_parse_from(["login", "prefix.dev", "--replace"]).is_err());
    let args = LoginArgs::try_parse_from([
        "login",
        "prefix.dev",
        "--oauth",
        "--replace",
        "--oauth-client-id",
        "new-client",
    ])
    .unwrap();
    assert!(args.oauth_replace);
    assert!(
        !LoginArgs::try_parse_from(["login", "prefix.dev", "--oauth"])
            .unwrap()
            .oauth_replace
    );
}

#[tokio::test]
async fn client_or_issuer_change_requires_explicit_replacement() {
    for change_issuer in [false, true] {
        let (storage, _) = storage();
        let old = grant(&config(&["previous:permission"]));
        storage.store("prefix.dev", &old).unwrap();
        let make_config = || {
            let mut config = config(&["new:permission"]);
            if change_issuer {
                config.issuer_url = "https://new.example".into();
            } else {
                config.client_id = "new-client".into();
            }
            config
        };
        let result =
            login_oauth_and_store("prefix.dev", &storage, make_config(), false, |_| async {
                panic!("implicit replacement must not authorize")
            })
            .await;
        assert!(matches!(
            result,
            Err(AuthenticationCLIError::OAuthScopes(
                oauth::EnsureScopesError::DifferentClient
            ))
        ));
        login_oauth_and_store(
            "prefix.dev",
            &storage,
            make_config(),
            true,
            |config| async move {
                assert!(!config.scopes.contains("previous:permission"));
                Ok(grant(&config))
            },
        )
        .await
        .unwrap();
        assert_eq!(
            storage.get("prefix.dev").unwrap(),
            Some(grant(&make_config()))
        );
    }
}

#[tokio::test]
async fn replacement_failure_or_partial_consent_keeps_original_credentials() {
    for partial in [false, true] {
        let (storage, _) = storage();
        let old = grant(&config(&["previous:permission"]));
        storage.store("prefix.dev", &old).unwrap();
        let mut desired = config(&["new:permission"]);
        desired.client_id = "new-client".into();
        let result = login_oauth_and_store(
            "prefix.dev",
            &storage,
            desired,
            true,
            |mut config| async move {
                if !partial {
                    return Err(oauth::OAuthError::Authorization("access_denied".into()));
                }
                config.scopes.remove("new:permission");
                Ok(grant(&config))
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(storage.get("prefix.dev").unwrap(), Some(old));
    }
}

#[tokio::test]
async fn unknown_legacy_grant_can_be_replaced_without_logout() {
    let (storage, _) = storage();
    let mut old = grant(&config(&["previous:permission"]));
    if let Authentication::OAuth {
        scopes,
        issuer_url,
        oidc,
        ..
    } = &mut old
    {
        *scopes = None;
        *issuer_url = None;
        *oidc = None;
    }
    storage.store("prefix.dev", &old).unwrap();
    login_oauth_and_store(
        "prefix.dev",
        &storage,
        config(&["new:permission"]),
        true,
        |config| async move { Ok(grant(&config)) },
    )
    .await
    .unwrap();
    assert_eq!(
        storage
            .get("prefix.dev")
            .unwrap()
            .unwrap()
            .oauth_scopes()
            .unwrap(),
        ["new:permission"]
    );
}
