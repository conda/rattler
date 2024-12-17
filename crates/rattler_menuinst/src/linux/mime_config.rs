use configparser::ini::Ini;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MimeConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Config parse error: {0}")]
    Parse(String),
}

#[derive(Debug)]
pub struct MimeConfig {
    config: Ini,
    path: PathBuf,
}

impl MimeConfig {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            // cs == case-sensitive
            config: Ini::new_cs(),
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn load(&mut self) -> Result<(), MimeConfigError> {
        if self.path.exists() {
            self.config
                .load(&self.path)
                .map_err(MimeConfigError::Parse)?;
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), MimeConfigError> {
        self.config.write(&self.path).map_err(MimeConfigError::Io)?;
        Ok(())
    }

    pub fn register_mime_type(&mut self, mime_type: &str, application: &str) {
        // Ensure sections exist
        self.config.set_default_section("Default Applications");
        self.config.set_default_section("Added Associations");

        // Only set default if not already set
        if self.config.get("Default Applications", mime_type).is_none() {
            self.config.set(
                "Default Applications",
                mime_type,
                Some(application.to_string()),
            );
        }

        // Update associations
        let existing = self
            .config
            .get("Added Associations", mime_type)
            .unwrap_or_default();

        let new_value = if !existing.is_empty() && !existing.contains(application) {
            format!("{existing};{application}")
        } else {
            application.to_string()
        };

        self.config
            .set("Added Associations", mime_type, Some(new_value));
    }

    pub fn deregister_mime_type(&mut self, mime_type: &str, application: &str) {
        for section in &["Default Applications", "Added Associations"] {
            if let Some(value) = self.config.get(section, mime_type) {
                if value == application {
                    self.config.remove_key(section, mime_type);
                } else if value.contains(application) {
                    let new_value: String = value
                        .split(';')
                        .filter(|&x| x != application)
                        .collect::<Vec<_>>()
                        .join(";");
                    self.config.set(section, mime_type, Some(new_value));
                }
            }

            // Remove empty sections
            if let Some(section_map) = self.config.get_map_ref().get(*section) {
                if section_map.is_empty() {
                    self.config.remove_section(section);
                }
            }
        }
    }

    pub fn get_default_application(&self, mime_type: &str) -> Option<String> {
        self.config.get("Default Applications", mime_type)
    }

    pub fn get_associations(&self, mime_type: &str) -> Vec<String> {
        self.config
            .get("Added Associations", mime_type)
            .map(|s| s.split(';').map(String::from).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_temp_config() -> (MimeConfig, NamedTempFile) {
        let file = NamedTempFile::new().unwrap();
        let config = MimeConfig::new(file.path());
        (config, file)
    }

    #[test]
    fn test_register_new_mime_type() {
        let (mut config, _file) = create_temp_config();

        config.register_mime_type("text/plain", "notepad.desktop");

        assert_eq!(
            config.get_default_application("text/plain"),
            Some("notepad.desktop".to_string())
        );
        assert_eq!(
            config.get_associations("text/plain"),
            vec!["notepad.desktop"]
        );
    }

    #[test]
    fn test_register_multiple_applications() {
        let (mut config, _file) = create_temp_config();

        config.register_mime_type("text/plain", "notepad.desktop");
        config.register_mime_type("text/plain", "gedit.desktop");

        // First application remains the default
        assert_eq!(
            config.get_default_application("text/plain"),
            Some("notepad.desktop".to_string())
        );

        // Both applications in associations
        let associations = config.get_associations("text/plain");
        assert!(associations.contains(&"notepad.desktop".to_string()));
        assert!(associations.contains(&"gedit.desktop".to_string()));
    }

    #[test]
    fn test_deregister_mime_type() {
        let (mut config, _file) = create_temp_config();

        config.register_mime_type("text/plain", "notepad.desktop");
        config.register_mime_type("text/plain", "gedit.desktop");
        config.deregister_mime_type("text/plain", "notepad.desktop");

        // notepad should be removed from associations
        let associations = config.get_associations("text/plain");
        assert!(!associations.contains(&"notepad.desktop".to_string()));
        assert!(associations.contains(&"gedit.desktop".to_string()));
    }

    #[test]
    fn test_load_and_save() -> Result<(), MimeConfigError> {
        let (mut config, file) = create_temp_config();

        config.register_mime_type("text/plain", "notepad.desktop");
        config.save()?;

        let mut new_config = MimeConfig::new(file.path());
        new_config.load()?;

        assert_eq!(
            new_config.get_default_application("text/plain"),
            Some("notepad.desktop".to_string())
        );
        Ok(())
    }

    fn get_config_contents(config: &MimeConfig) -> String {
        config.save().unwrap();
        std::fs::read_to_string(&config.path).unwrap()
    }

    #[test]
    fn test_mime_config_snapshots() {
        let (mut config, _file) = create_temp_config();

        // Test progressive changes to the config
        config.register_mime_type("text/plain", "notepad.desktop");
        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=notepad.desktop
        [Default Applications]
        text/plain=notepad.desktop
        "###);

        config.register_mime_type("text/plain", "gedit.desktop");
        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=notepad.desktop;gedit.desktop
        [Default Applications]
        text/plain=notepad.desktop
        "###);

        config.register_mime_type("application/pdf", "pdf-reader.desktop");
        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=notepad.desktop;gedit.desktop
        application/pdf=pdf-reader.desktop
        [Default Applications]
        text/plain=notepad.desktop
        application/pdf=pdf-reader.desktop
        "###);

        config.deregister_mime_type("text/plain", "notepad.desktop");
        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=gedit.desktop
        application/pdf=pdf-reader.desktop
        [Default Applications]
        application/pdf=pdf-reader.desktop
        "###);
    }

    #[test]
    fn test_complex_mime_associations_snapshot() {
        let (mut config, _file) = create_temp_config();

        // Add multiple mime types with multiple applications
        let test_cases = [
            (
                "text/plain",
                vec!["notepad.desktop", "gedit.desktop", "vim.desktop"],
            ),
            (
                "application/pdf",
                vec!["pdf-reader.desktop", "browser.desktop"],
            ),
            ("image/jpeg", vec!["image-viewer.desktop", "gimp.desktop"]),
        ];

        for (mime_type, apps) in test_cases.iter() {
            for app in apps {
                config.register_mime_type(mime_type, app);
            }
        }

        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=notepad.desktop;gedit.desktop;vim.desktop
        application/pdf=pdf-reader.desktop;browser.desktop
        image/jpeg=image-viewer.desktop;gimp.desktop
        [Default Applications]
        text/plain=notepad.desktop
        application/pdf=pdf-reader.desktop
        image/jpeg=image-viewer.desktop
        "###);

        // Remove some applications
        config.deregister_mime_type("text/plain", "gedit.desktop");
        config.deregister_mime_type("application/pdf", "pdf-reader.desktop");

        insta::assert_snapshot!(get_config_contents(&config), @r###"
        text/plain=notepad.desktop;vim.desktop
        application/pdf=browser.desktop
        image/jpeg=image-viewer.desktop;gimp.desktop
        [Default Applications]
        text/plain=notepad.desktop
        image/jpeg=image-viewer.desktop
        "###);
    }
}
