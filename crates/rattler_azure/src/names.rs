use crate::AzureUrlError;

/// The charset admits no `-`, so an account name cannot be option-shaped
/// (`--as-user`, `-o`) in the `az` argv the SAS mint builds, which is why the mint
/// takes this type rather than a `&str`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: &str) -> Result<Self, AzureUrlError> {
        let valid = (3..=24).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());

        if valid {
            Ok(Self(name.to_string()))
        } else {
            Err(AzureUrlError::InvalidAccountName(name.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Exists for the same reason as [`AccountName`].
///
/// It is also the key of an `auth` table in `azure-options`, hence the hash and
/// string serde bridge. Azure's rules make a container name lowercase by
/// construction, so unlike a host there is only one spelling of one container.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(try_from = "String", into = "String")
)]
pub struct ContainerName(String);

impl ContainerName {
    pub fn new(name: &str) -> Result<Self, AzureUrlError> {
        let valid = (3..=63).contains(&name.len())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--");

        if valid {
            Ok(Self(name.to_string()))
        } else {
            Err(AzureUrlError::InvalidContainerName(name.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContainerName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for ContainerName {
    type Err = AzureUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for ContainerName {
    type Error = AzureUrlError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<ContainerName> for String {
    fn from(container: ContainerName) -> Self {
        container.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_components_are_rejected() {
        assert!(AccountName::new("").is_err());
        assert!(ContainerName::new("").is_err());
    }
}
