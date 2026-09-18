//! Shared command-line arguments for constraining a Sigstore publisher.

use std::str::FromStr;

use rattler_sigstore::{Issuer, Publisher};
use url::Url;

/// Command-line arguments that select an accepted Sigstore publisher.
#[derive(Debug, Default, clap::Args)]
pub struct PublisherArgs {
    /// Required signing certificate identity (Subject Alternative Name).
    ///
    /// `*` matches any sequence of characters. For example,
    /// `https://github.com/org/repo/*` accepts every workflow and ref in a
    /// GitHub repository.
    #[clap(long, visible_alias = "trusted-publisher", value_name = "PATTERN")]
    identity: Option<String>,

    /// Required OIDC issuer for the signing certificate.
    ///
    /// Accepts an exact issuer URL, or `github` and `gitlab` as shorthands for
    /// the GitHub Actions and GitLab CI issuers.
    #[clap(long, visible_alias = "trusted-issuer", value_name = "ISSUER")]
    issuer: Option<IssuerArg>,
}

impl PublisherArgs {
    /// Builds the publisher described by the arguments.
    pub fn publisher(&self) -> Publisher {
        let mut publisher = Publisher::new();
        if let Some(identity) = &self.identity {
            publisher = publisher.with_identity(identity.as_str());
        }
        if let Some(issuer) = &self.issuer {
            publisher = publisher.with_issuer(issuer.0.clone());
        }
        publisher
    }
}

#[derive(Debug, Clone)]
struct IssuerArg(Issuer);

impl FromStr for IssuerArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let issuer = match value {
            "github" => Issuer::github_actions(),
            "gitlab" => Issuer::gitlab(),
            url => {
                let parsed = Url::parse(url).map_err(|err| {
                    format!(
                        "invalid issuer {url:?}: expected `github`, `gitlab`, or an issuer URL ({err})"
                    )
                })?;
                if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
                    return Err(format!(
                        "invalid issuer {url:?}: expected `github`, `gitlab`, or an HTTP(S) issuer URL"
                    ));
                }
                Issuer::new(url)
            }
        };
        Ok(Self(issuer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(clap::Parser)]
    struct Cli {
        #[clap(flatten)]
        publisher: PublisherArgs,
    }

    #[test]
    fn issuer_shorthands_expand_to_oidc_issuers() {
        assert_eq!(
            "github".parse::<IssuerArg>().unwrap().0,
            Issuer::github_actions()
        );
        assert_eq!("gitlab".parse::<IssuerArg>().unwrap().0, Issuer::gitlab());
    }

    #[test]
    fn issuer_accepts_explicit_urls() {
        let issuer = "https://gitlab.example.com".parse::<IssuerArg>().unwrap();
        assert_eq!(issuer.0.as_str(), "https://gitlab.example.com");
        for invalid in ["not-an-issuer", "mailto:build@example.com"] {
            assert!(invalid.parse::<IssuerArg>().is_err());
        }
    }

    #[test]
    fn cli_accepts_publisher_constraints_and_aliases() {
        for (identity_flag, issuer_flag) in [
            ("--identity", "--issuer"),
            ("--trusted-publisher", "--trusted-issuer"),
        ] {
            let cli = <Cli as clap::Parser>::try_parse_from([
                "test",
                identity_flag,
                "https://github.com/org/repo/*",
                issuer_flag,
                "github",
            ])
            .unwrap();
            assert!(cli.publisher.publisher().matches(
                Some("https://github.com/org/repo/workflow"),
                Some(Issuer::github_actions().as_str()),
            ));
        }
    }
}
