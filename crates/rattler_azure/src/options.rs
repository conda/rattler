//! Per-endpoint options: which containers a credential may attach to, and the
//! wire scheme.

use crate::ContainerName;

/// Whether credentials may attach to requests for a container.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "bool", into = "bool")
)]
pub enum Auth {
    /// Send requests unsigned
    #[default]
    Anonymous,

    /// Resolve a credential and sign with it
    DefaultChain,
}

impl From<bool> for Auth {
    fn from(value: bool) -> Self {
        if value {
            Auth::DefaultChain
        } else {
            Auth::Anonymous
        }
    }
}

impl From<Auth> for bool {
    fn from(value: Auth) -> Self {
        matches!(value, Auth::DefaultChain)
    }
}

impl Auth {
    pub fn is_granted(self) -> bool {
        matches!(self, Auth::DefaultChain)
    }
}

/// The wire scheme an `az://` channel URL is rewritten to when a request is sent.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "lowercase")
)]
pub enum AzureScheme {
    #[default]
    Https,

    Http,
}

impl AzureScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            AzureScheme::Https => "https",
            AzureScheme::Http => "http",
        }
    }
}

impl std::fmt::Display for AzureScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the fetch middleware needs to send a request.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AzureFetchOptions {
    pub auth: Auth,

    pub scheme: AzureScheme,
}

/// The options of one endpoint entry.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(rename_all = "kebab-case", default, deny_unknown_fields)
)]
pub struct AzureEndpointOptions {
    scheme: AzureScheme,

    #[cfg_attr(
        feature = "serde",
        serde(skip_serializing_if = "indexmap::IndexMap::is_empty")
    )]
    auth: indexmap::IndexMap<ContainerName, Auth>,
}

impl AzureEndpointOptions {
    pub fn new(auth: impl IntoIterator<Item = (ContainerName, Auth)>, scheme: AzureScheme) -> Self {
        Self {
            scheme,
            auth: auth.into_iter().collect(),
        }
    }

    pub fn scheme(&self) -> AzureScheme {
        self.scheme
    }

    pub fn fetch(&self, container: Option<&ContainerName>) -> AzureFetchOptions {
        AzureFetchOptions {
            auth: container
                .and_then(|container| self.auth.get(container))
                .copied()
                .unwrap_or_default(),
            scheme: self.scheme,
        }
    }

    pub fn grants(&self) -> impl Iterator<Item = (&ContainerName, Auth)> {
        self.auth.iter().map(|(container, auth)| (container, *auth))
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn container(name: &str) -> ContainerName {
        ContainerName::new(name).expect("test container name")
    }

    #[test]
    fn enums_round_trip_as_toml_bools() {
        let opts = AzureEndpointOptions::new(
            [
                (container("releases"), Auth::DefaultChain),
                (container("public"), Auth::Anonymous),
            ],
            AzureScheme::Http,
        );

        let written = toml::to_string(&opts).unwrap();
        assert!(written.contains("releases = true"), "{written}");
        assert!(written.contains("public = false"), "{written}");
        assert!(written.contains(r#"scheme = "http""#), "{written}");
        assert!(!written.contains("DefaultChain"), "{written}");

        assert_eq!(toml::from_str::<AzureEndpointOptions>(&written), Ok(opts));

        // The default writes no `auth` table, and an empty document reads back
        // as the default, which grants no container.
        let default = toml::to_string(&AzureEndpointOptions::default()).unwrap();
        assert!(!default.contains("auth"), "{default}");
        let empty: AzureEndpointOptions = toml::from_str("").unwrap();
        assert_eq!(empty, AzureEndpointOptions::default());
        assert_eq!(
            empty.fetch(Some(&container("releases"))),
            AzureFetchOptions::default()
        );
    }

    #[test]
    fn a_grant_applies_to_one_container_only() {
        let opts: AzureEndpointOptions = toml::from_str(
            r#"
            [auth]
            releases = true
            public = false
            "#,
        )
        .unwrap();

        assert!(opts.fetch(Some(&container("releases"))).auth.is_granted());
        assert!(!opts.fetch(Some(&container("public"))).auth.is_granted());
        assert!(!opts.fetch(Some(&container("staging"))).auth.is_granted());

        assert!(!opts.fetch(None).auth.is_granted());

        // `grants` reports what the file says, explicit `false` included.
        let grants: HashMap<_, _> = opts.grants().collect();
        assert_eq!(
            grants,
            HashMap::from([
                (&container("public"), Auth::Anonymous),
                (&container("releases"), Auth::DefaultChain),
            ])
        );
    }

    #[test]
    fn an_unusable_container_key_is_rejected() {
        let err = toml::from_str::<AzureEndpointOptions>("[auth]\nReleases = true\n")
            .expect_err("uppercase is not a legal container name");
        assert!(err.to_string().contains("Releases"), "{err}");
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        for document in [
            "[Auth]\nreleases = true\n",
            "[authz]\nreleases = true\n",
            "path-style = true\n",
        ] {
            assert!(
                toml::from_str::<AzureEndpointOptions>(document).is_err(),
                "silently ignored: {document}"
            );
        }
    }
}
