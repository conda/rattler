use rattler_conda_types::{
    ChannelUrl, PackageName,
    utils::{InvalidPathComponentError, ensure_safe_path_component},
};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};

/// Identifies one plugin's detection result in the cache.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct CacheKey {
    channel: ChannelUrl,
    plugin: PackageName,
    environment_sha256: Sha256Hash,
}

impl CacheKey {
    /// Identifies `plugin` from `channel`, as installed in the environment
    /// identified by `environment_sha256`.
    pub fn new(channel: ChannelUrl, plugin: PackageName, environment_sha256: Sha256Hash) -> Self {
        Self {
            channel,
            plugin,
            environment_sha256,
        }
    }

    /// The file name this key's entry is stored under.
    pub fn to_file_name(&self) -> Result<String, InvalidPathComponentError> {
        let plugin = self.plugin.as_normalized();
        ensure_safe_path_component(plugin)?;
        Ok(format!("{plugin}-{}.json", hex::encode(self.digest())))
    }

    /// A digest over every part of the key, so two keys differing in any of them
    /// land in different files.
    fn digest(&self) -> Sha256Hash {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.channel.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.plugin.as_normalized().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.environment_sha256.as_slice());
        compute_bytes_digest::<Sha256>(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(channel: &str, plugin: &str, env: u8) -> CacheKey {
        CacheKey::new(
            url::Url::parse(channel).unwrap().into(),
            PackageName::new_unchecked(plugin),
            compute_bytes_digest::<Sha256>([env]),
        )
    }

    #[test]
    fn the_same_key_always_names_the_same_file() {
        let a = key("https://prefix.dev/org/", "cuda-detect", 1);
        let b = key("https://prefix.dev/org/", "cuda-detect", 1);
        assert_eq!(a.to_file_name().unwrap(), b.to_file_name().unwrap());
    }

    #[test]
    fn every_part_of_the_key_separates_entries() {
        let base = key("https://prefix.dev/org/", "cuda-detect", 1);
        for other in [
            key("https://prefix.dev/other/", "cuda-detect", 1),
            key("https://prefix.dev/org/", "rocm-detect", 1),
            key("https://prefix.dev/org/", "cuda-detect", 2),
        ] {
            assert_ne!(
                base.to_file_name().unwrap(),
                other.to_file_name().unwrap(),
                "keys differing in one part must not share a file"
            );
        }
    }

    #[test]
    fn the_file_name_starts_with_the_plugin_name() {
        let name = key("https://prefix.dev/org/", "cuda-detect", 1)
            .to_file_name()
            .unwrap();
        assert!(name.starts_with("cuda-detect-"), "{name}");
        assert!(name.ends_with(".json"), "{name}");
    }

    #[test]
    fn a_plugin_name_cannot_escape_the_cache_directory() {
        let escaping = CacheKey::new(
            url::Url::parse("https://prefix.dev/org/").unwrap().into(),
            PackageName::new_unchecked("../../etc/passwd"),
            compute_bytes_digest::<Sha256>([1]),
        );
        assert!(escaping.to_file_name().is_err());
    }
}
