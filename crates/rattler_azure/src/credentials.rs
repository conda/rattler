use secrecy::SecretString;

#[derive(Clone, Debug)]
pub enum AzureCredentials {
    AccountKey(SecretString),
    SasToken(SecretString),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_secret() {
        for creds in [
            AzureCredentials::AccountKey("supersecretkey".into()),
            AzureCredentials::SasToken("sig=deadbeef".into()),
        ] {
            let out = format!("{creds:?}");
            assert!(out.contains("REDACTED"), "not redacted: {out}");
            assert!(!out.contains("supersecret"));
            assert!(!out.contains("deadbeef"));
        }
    }
}
