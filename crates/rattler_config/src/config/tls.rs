use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Error returned when parsing [`TlsRootCerts`] from a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseTlsRootCertsError {
    value: String,
}

impl fmt::Display for ParseTlsRootCertsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown TLS root certificate setting `{}` (expected `webpki` or `system`)",
            self.value
        )
    }
}

impl std::error::Error for ParseTlsRootCertsError {}

/// Which root certificates to use for HTTPS connections.
///
/// This is a hint to the HTTPS client. Whether it has any effect depends on
/// the TLS backend the consumer is built with. Consumers should keep treating
/// `SSL_CERT_FILE` / `SSL_CERT_DIR` as overrides.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TlsRootCerts {
    /// Use bundled Mozilla root certificates.
    Webpki,

    /// Use the system's native certificate store.
    ///
    /// Legacy `"native"` and `"all"` values deserialize as `System`.
    #[default]
    #[serde(alias = "native", alias = "all")]
    System,
}

impl FromStr for TlsRootCerts {
    type Err = ParseTlsRootCertsError;

    /// Parse the same canonical strings and legacy aliases as `Deserialize`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "webpki" => Ok(Self::Webpki),
            "system" | "native" | "all" => Ok(Self::System),
            value => Err(ParseTlsRootCertsError {
                value: value.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_system() {
        assert_eq!(TlsRootCerts::default(), TlsRootCerts::System);
    }

    #[test]
    fn deserialize_canonical() {
        assert_eq!(
            serde_json::from_str::<TlsRootCerts>("\"webpki\"").unwrap(),
            TlsRootCerts::Webpki
        );
        assert_eq!(
            serde_json::from_str::<TlsRootCerts>("\"system\"").unwrap(),
            TlsRootCerts::System
        );
    }

    /// Legacy `"native"` resolves to `System`.
    #[test]
    fn deserialize_legacy_native_alias() {
        assert_eq!(
            serde_json::from_str::<TlsRootCerts>("\"native\"").unwrap(),
            TlsRootCerts::System
        );
    }

    /// Legacy `"all"` resolves to `System`.
    #[test]
    fn deserialize_legacy_all_alias() {
        assert_eq!(
            serde_json::from_str::<TlsRootCerts>("\"all\"").unwrap(),
            TlsRootCerts::System
        );
    }

    #[test]
    fn from_str_matches_deserialize_values() {
        assert_eq!(
            "webpki".parse::<TlsRootCerts>().unwrap(),
            TlsRootCerts::Webpki
        );
        assert_eq!(
            "system".parse::<TlsRootCerts>().unwrap(),
            TlsRootCerts::System
        );
        assert_eq!(
            "native".parse::<TlsRootCerts>().unwrap(),
            TlsRootCerts::System
        );
        assert_eq!("all".parse::<TlsRootCerts>().unwrap(), TlsRootCerts::System);
        assert!("unknown".parse::<TlsRootCerts>().is_err());
    }

    /// Serializing always emits canonical lowercase spelling.
    #[test]
    fn serialize_canonical_only() {
        assert_eq!(
            serde_json::to_string(&TlsRootCerts::Webpki).unwrap(),
            "\"webpki\""
        );
        assert_eq!(
            serde_json::to_string(&TlsRootCerts::System).unwrap(),
            "\"system\""
        );
    }
}
