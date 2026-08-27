use indexmap::IndexMap;
use rattler_azure::{AzureEndpointKey, AzureEndpointOptions, AzureHost};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Azure Blob channel options, keyed by [`AzureEndpointKey`] with
/// [`AzureEndpointOptions`] as the entry.
///
/// Entries are **user-scoped by contract**. Read from a project manifest, a
/// checked-out repository could name a host and be sent ambient credentials.
#[derive(Default, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AzureOptionsMap(IndexMap<AzureEndpointKey, AzureEndpointOptions>);

impl AzureOptionsMap {
    pub fn get(&self, key: &AzureEndpointKey) -> AzureEndpointOptions {
        self.0.get(key).cloned().unwrap_or_default()
    }

    pub fn contains(&self, key: &AzureEndpointKey) -> bool {
        self.0.contains_key(key)
    }
}

/// Both spellings reach serde, which silently keeps whichever the table iterated
/// last — byte order of the raw keys, not the order they were written — so one
/// entry's grants vanish and Azure reports the private container as a 404. TOML's
/// own duplicate-key check runs on the raw text, so it cannot see the collision.
///
/// A host carrying both readings of its account is refused on the same terms: only
/// one of `H` and `H/<account>` can describe an endpoint, and longest-match would
/// pick the path-style one without saying so.
pub(crate) fn ensure_no_colliding_keys(document: &toml::Table) -> Result<(), String> {
    let Some(table) = document
        .get("azure-options")
        .and_then(toml::Value::as_table)
    else {
        return Ok(());
    };

    let mut seen: IndexMap<AzureEndpointKey, &String> = IndexMap::new();
    let mut reading: IndexMap<AzureHost, (&String, bool)> = IndexMap::new();
    for written in table.keys() {
        // An unparsable key is serde's error to report, not ours.
        let Ok(key) = AzureEndpointKey::parse(written) else {
            continue;
        };
        if let Some(first) = seen.insert(key.clone(), written) {
            return Err(format!(
                "`azure-options` names one endpoint twice: \"{first}\" and \"{written}\" are both \
                 `{key}`"
            ));
        }

        let host_style = matches!(key, AzureEndpointKey::HostStyle(_));
        if let Some((first, first_host_style)) =
            reading.insert(key.host().clone(), (written, host_style))
            && first_host_style != host_style
        {
            return Err(format!(
                "`azure-options` reads `{}` both ways: \"{first}\" and \"{written}\" disagree on \
                 whether the storage account is the host's first label or a path segment",
                key.host()
            ));
        }
    }
    Ok(())
}

impl Config for AzureOptionsMap {
    fn is_default(&self) -> bool {
        self.0.is_empty()
    }

    fn merge_config(self, other: &Self) -> Result<Self, super::MergeError> {
        // An entry replaces wholesale rather than merging field by field. Merging
        // let two individually-valid files produce a combination neither wrote: a
        // system file granting a container over https, plus a user file setting
        // only `scheme = "http"` on the same key, used to yield a cleartext grant.
        // An entry is one unit — its scheme and grants describe one endpoint — so
        // the layer that names a key owns it.
        let mut merged = self.0;
        merged.extend(
            other
                .0
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Ok(AzureOptionsMap(merged))
    }

    fn keys(&self) -> Vec<String> {
        // Quoted, because a key can carry dots, a colon and a slash, none of which
        // a bare TOML key holds — and the path is what the user passes to `config
        // set`/`unset`.
        //
        // The per-container grants are listed as their own keys so each is
        // separately unsettable. There is deliberately no `."<key>".auth` key: the
        // path exists only as a table of containers, so `config set
        // azure-options."<key>".auth true` has nowhere to land — which is the
        // point, since that is the one edit whose blast radius would be the whole
        // account.
        self.0
            .iter()
            .flat_map(|(key, options)| {
                let key = toml::Value::from(key.to_string()).to_string();
                let grants = options
                    .grants()
                    .map(|(container, _)| {
                        let container = toml::Value::from(container.to_string()).to_string();
                        format!("{key}.auth.{container}")
                    })
                    .collect::<Vec<_>>();
                std::iter::once(key).chain(grants)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rattler_azure::{Auth, AzureScheme, ContainerName};

    use super::*;

    fn key(written: &str) -> AzureEndpointKey {
        AzureEndpointKey::parse(written).expect("test key should parse")
    }

    fn container(name: &str) -> ContainerName {
        ContainerName::new(name).expect("test container name")
    }

    #[test]
    fn table_parses_and_absent_keys_default() {
        let map: AzureOptionsMap = toml::from_str(
            r#"
            ["mycompany.blob.core.windows.net".auth]
            releases = true

            ["127.0.0.1:10000/devstoreaccount1"]
            scheme = "http"

            ["127.0.0.1:10000/devstoreaccount1".auth]
            general = true
            "#,
        )
        .unwrap();

        let real = map.get(&key("mycompany.blob.core.windows.net"));
        assert_eq!(
            real.fetch(Some(&container("releases"))).auth,
            Auth::DefaultChain
        );
        assert_eq!(real.scheme(), AzureScheme::Https);

        let azurite = map.get(&key("127.0.0.1:10000/devstoreaccount1"));
        assert_eq!(
            azurite.fetch(Some(&container("general"))).auth,
            Auth::DefaultChain
        );
        assert_eq!(azurite.scheme(), AzureScheme::Http);

        assert!(!real.fetch(Some(&container("public"))).auth.is_granted());
        let unlisted = map.get(&key("someoneelse.blob.core.windows.net"));
        assert!(
            !unlisted
                .fetch(Some(&container("releases")))
                .auth
                .is_granted()
        );
        assert_eq!(unlisted, AzureEndpointOptions::default());
    }

    #[test]
    fn two_accounts_on_one_host_have_independent_grants() {
        let map: AzureOptionsMap = toml::from_str(
            r#"
            ["proxy.internal/accta".auth]
            general = true

            ["proxy.internal/acctb".auth]
            general = false
            "#,
        )
        .unwrap();

        assert!(
            map.get(&key("proxy.internal/accta"))
                .fetch(Some(&container("general")))
                .auth
                .is_granted()
        );
        assert!(
            !map.get(&key("proxy.internal/acctb"))
                .fetch(Some(&container("general")))
                .auth
                .is_granted()
        );
    }

    #[test]
    fn merge_replaces_the_endpoint_wholesale() {
        let base: AzureOptionsMap =
            toml::from_str("[\"host.example\"]\nscheme = \"http\"\n").unwrap();
        let over: AzureOptionsMap =
            toml::from_str("[\"host.example\".auth]\nreleases = true\n").unwrap();

        let merged = base.merge_config(&over).unwrap();
        let entry = merged.get(&key("host.example"));
        assert_eq!(entry.scheme(), AzureScheme::Https);
        assert!(entry.fetch(Some(&container("releases"))).auth.is_granted());
    }

    #[test]
    fn merge_replaces_the_grant_table_wholesale() {
        let system: AzureOptionsMap =
            toml::from_str("[\"host.example\".auth]\nreleases = true\nstaging = true\n").unwrap();
        let user: AzureOptionsMap =
            toml::from_str("[\"host.example\".auth]\ninternal = true\n").unwrap();

        let entry = system
            .merge_config(&user)
            .unwrap()
            .get(&key("host.example"));
        assert!(entry.fetch(Some(&container("internal"))).auth.is_granted());
        assert!(
            !entry.fetch(Some(&container("releases"))).auth.is_granted(),
            "a grant the higher-precedence file omits must not survive the merge"
        );
    }

    #[test]
    fn keys_are_normalized_the_same_way_lookups_are() {
        let written = "MyCompany.blob.core.windows.net";
        let looked_up = "mycompany.blob.core.windows.net";
        let map: AzureOptionsMap =
            toml::from_str(&format!("[\"{written}\".auth]\nreleases = true\n")).unwrap();

        assert!(
            map.get(&key(looked_up))
                .fetch(Some(&container("releases")))
                .auth
                .is_granted(),
            "the grant written as `{written}` did not apply to `{looked_up}`"
        );
        assert_eq!(
            map.keys(),
            vec![
                format!("\"{looked_up}\""),
                format!("\"{looked_up}\".auth.\"releases\""),
            ],
        );
        let written_back = toml::to_string(&map).unwrap();
        assert!(
            written_back.contains(&format!("[\"{looked_up}\"")),
            "{written} was written back as {written_back}"
        );
    }

    #[test]
    fn a_document_naming_one_endpoint_twice_is_refused() {
        for document in [
            r#"
[azure-options."acct.blob.example".auth]
releases = false

[azure-options."ACCT.blob.example.".auth]
releases = true
"#,
            r#"
[azure-options."proxy.internal/accta".auth]
releases = false

[azure-options."Proxy.Internal./accta".auth]
releases = true
"#,
        ] {
            let error = ensure_no_colliding_keys(&document.parse().unwrap())
                .expect_err("a collision must be reported");
            assert!(error.contains("names one endpoint twice"), "{error}");
        }
    }

    #[test]
    fn a_host_read_both_ways_is_refused() {
        let document = r#"
[azure-options."proxy.internal".auth]
accta = true

[azure-options."proxy.internal/accta".auth]
general = true
"#;
        let error = ensure_no_colliding_keys(&document.parse().unwrap())
            .expect_err("both readings of one host must be reported");
        assert!(error.contains("proxy.internal"), "{error}");
    }

    #[test]
    fn two_path_style_keys_on_one_host_are_allowed() {
        let document = r#"
[azure-options."proxy.internal/accta".auth]
general = true

[azure-options."proxy.internal/acctb".auth]
general = true
"#;
        assert!(ensure_no_colliding_keys(&document.parse().unwrap()).is_ok());
    }

    #[test]
    fn a_key_past_the_account_is_rejected() {
        let err = toml::from_str::<AzureOptionsMap>(
            "[\"acct.blob.example/general/noarch\".auth]\nreleases = true\n",
        )
        .expect_err("a key past the account must be rejected");
        assert!(
            err.to_string().contains("acct.blob.example/general/noarch"),
            "{err}"
        );
    }
}
