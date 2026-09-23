use super::*;
use rattler_networking::authentication_storage::backends::memory::MemoryStorage;

fn grant(config: &oauth::OAuthConfig) -> Authentication {
    Authentication::OAuth {
        access_token: "opaque-test-access-token".into(),
        refresh_token: Some("test-refresh".into()),
        expires_at: None,
        token_endpoint: "https://prefix.dev/token".into(),
        revocation_endpoint: None,
        client_id: config.client_id.clone(),
        issuer_url: Some(config.issuer_url.clone()),
        scopes: Some(config.scopes.iter().cloned().collect()),
    }
}

fn storage() -> AuthenticationStorage {
    let mut storage = AuthenticationStorage::empty();
    storage.add_backend(std::sync::Arc::new(MemoryStorage::new()));
    storage
}

#[test]
fn repeatable_cli_scopes_extend_defaults_and_deduplicate() {
    let args = LoginArgs::try_parse_from([
        "login",
        "prefix.dev",
        "--oauth",
        "--oauth-scope",
        "custom:read",
        "--oauth-scope",
        "custom:write",
        "--oauth-scope",
        "custom:read",
    ])
    .unwrap();
    let defaults = default_oauth_config_for_host(&args.host).unwrap();
    let scopes = login_oauth_scopes(Some(&defaults), &args.oauth_scopes);
    for scope in &defaults.scopes {
        assert!(scopes.contains(scope));
    }
    assert!(scopes.contains("custom:read"));
    assert!(scopes.contains("custom:write"));
    assert_eq!(scopes.len(), defaults.scopes.len() + 2);
    let generic = login_oauth_scopes(None, &args.oauth_scopes);
    for scope in oauth::DEFAULT_OAUTH_SCOPES {
        assert!(generic.contains(*scope));
    }
    assert!(generic.contains("custom:read"));
    assert!(!generic.contains("channel:upload"));
}

#[tokio::test]
async fn explicit_login_unions_defaults_existing_and_custom_scopes_and_keeps_key() {
    for (host, key) in [
        ("prefix.dev", "prefix.dev"),
        ("prefix.dev", "*.prefix.dev"),
        ("repo.prefix.dev", "*.prefix.dev"),
    ] {
        let storage = storage();
        let current =
            grant(&oauth_config_for_host("prefix.dev", &["previous:permission"]).unwrap());
        storage.store(key, &current).unwrap();
        let mut config = oauth_config_for_host("prefix.dev", &[]).unwrap();
        config.scopes = login_oauth_scopes(
            default_oauth_config_for_host("prefix.dev").as_ref(),
            &["new:permission".into()],
        );
        config.flow = oauth::OAuthFlow::DeviceCode;
        login_oauth_and_store(
            &format!("https://{host}/"),
            &storage,
            config,
            |config| async move {
                assert_eq!(config.flow, oauth::OAuthFlow::DeviceCode);
                for scope in [
                    "openid",
                    "profile",
                    "offline_access",
                    "channel:read",
                    "channel:upload",
                    "previous:permission",
                    "new:permission",
                ] {
                    assert!(config.scopes.contains(scope), "missing {scope}");
                }
                Ok(grant(&config))
            },
        )
        .await
        .unwrap();
        let updated = storage.get(key).unwrap().unwrap();
        assert!(
            updated
                .oauth_scopes()
                .unwrap()
                .contains(&"new:permission".into())
        );
        if key.starts_with('*') {
            assert_eq!(storage.get(host).unwrap(), None);
        }
    }
}

#[tokio::test]
async fn explicit_login_still_runs_when_permissions_are_already_granted() {
    let storage = storage();
    let config = oauth_config_for_host("prefix.dev", &["custom:read"]).unwrap();
    storage.store("prefix.dev", &grant(&config)).unwrap();
    let called = std::cell::Cell::new(false);
    login_oauth_and_store("prefix.dev", &storage, config, |config| {
        called.set(true);
        async move { Ok(grant(&config)) }
    })
    .await
    .unwrap();
    assert!(called.get());
}

#[tokio::test]
async fn failed_or_partial_consent_never_replaces_stored_credentials() {
    for partial in [false, true] {
        let storage = storage();
        let current =
            grant(&oauth_config_for_host("prefix.dev", &["previous:permission"]).unwrap());
        storage.store("prefix.dev", &current).unwrap();
        let config = oauth_config_for_host("prefix.dev", &["custom:read"]).unwrap();
        let result =
            login_oauth_and_store("prefix.dev", &storage, config, |mut config| async move {
                if !partial {
                    return Err(oauth::OAuthError::Authorization("access_denied".into()));
                }
                config.scopes.remove("previous:permission");
                Ok(grant(&config))
            })
            .await;
        assert!(result.is_err());
        assert_eq!(storage.get("prefix.dev").unwrap(), Some(current));
    }
}
