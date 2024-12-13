use chrono::Utc;
use fs_err::{self as fs, File};
use quick_xml::events::Event;
use quick_xml::{Reader, Writer};
use std::io::Write;
use std::path::PathBuf;

use crate::{slugify, MenuInstError};

pub struct MenuXml {
    menu_config_location: PathBuf,
    system_menu_config_location: PathBuf,
    name: String,
    mode: String,
}

impl MenuXml {
    pub fn new(
        menu_config_location: PathBuf,
        system_menu_config_location: PathBuf,
        name: String,
        mode: String,
    ) -> Result<Self, MenuInstError> {
        Ok(Self {
            menu_config_location,
            system_menu_config_location,
            name,
            mode,
        })
    }

    fn is_target_menu(&self, mut reader: Reader<&[u8]>) -> Result<bool, MenuInstError> {
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) if e.name().as_ref() == b"Name" => {
                    if let Event::Text(t) = reader.read_event_into(&mut buf)? {
                        return Ok(t.unescape()?.into_owned() == self.name);
                    }
                }
                Event::End(e) if e.name().as_ref() == b"Menu" => break,
                _ => (),
            }
        }
        Ok(false)
    }

    fn contents(&self) -> Result<String, MenuInstError> {
        Ok(fs::read_to_string(&self.menu_config_location)?)
    }

    pub fn remove_menu(&self) -> Result<(), MenuInstError> {
        tracing::info!(
            "Editing {} to remove {} config",
            self.menu_config_location.display(),
            self.name
        );

        let contents = self.contents()?;
        let mut reader = Reader::from_str(&contents);
        reader.config_mut().trim_text(true);

        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
        let mut buf = Vec::new();
        let mut skip_menu = false;
        let mut depth = 0;

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => {
                    if e.name().as_ref() == b"Menu" {
                        depth += 1;
                        if depth == 1 {
                            // Always write the root Menu element
                            writer.write_event(Event::Start(e))?;
                        } else {
                            // Check if this is our target menu

                            if self.is_target_menu(reader.clone())? {
                                skip_menu = true;
                            } else {
                                writer.write_event(Event::Start(e))?;
                            }
                        }
                    } else if !skip_menu {
                        writer.write_event(Event::Start(e))?;
                    }
                }
                Event::End(e) => {
                    if skip_menu && e.name().as_ref() == b"Menu" {
                        skip_menu = false;
                    } else if !skip_menu {
                        writer.write_event(Event::End(e))?;
                    }
                }
                Event::Eof => break,
                e => {
                    if !skip_menu {
                        writer.write_event(e)?;
                    }
                }
            }
            buf.clear();
        }

        self.write_menu_file(&writer.into_inner())
    }

    pub fn has_menu(&self) -> Result<bool, MenuInstError> {
        let contents = self.contents()?;
        let mut reader = Reader::from_str(&contents);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) if e.name().as_ref() == b"Menu" => {
                    if self.is_target_menu(reader.clone())? {
                        return Ok(true);
                    }
                }
                Event::Eof => break,
                _ => (),
            }
            buf.clear();
        }
        Ok(false)
    }

    pub fn add_menu(&self) -> Result<(), MenuInstError> {
        tracing::info!(
            "Editing {} to add {} config",
            self.menu_config_location.display(),
            self.name
        );

        let mut content = fs::read_to_string(&self.menu_config_location)?;
        // let insert_pos = content.rfind("</Menu>").ok_or_else(|| anyhow!("Invalid XML"))?;
        let insert_pos = content.rfind("</Menu>").unwrap();

        let menu_entry = format!(
            r#"  <Menu>
    <Name>{}</Name>
    <Directory>{}.directory</Directory>
    <Include>
      <Category>{}</Category>
    </Include>
  </Menu>
"#,
            self.name,
            slugify(&self.name),
            self.name
        );

        content.insert_str(insert_pos, &menu_entry);
        self.write_menu_file(content.as_bytes())
    }

    pub fn is_valid_menu_file(&self) -> bool {
        if let Ok(contents) = self.contents() {
            let reader = Reader::from_str(&contents);
            let mut buf = Vec::new();
            let mut reader = reader;
            loop {
                match reader.read_event_into(&mut buf) {
                    Ok(Event::Start(e)) => {
                        if e.name().as_ref() == b"Menu" {
                            return true;
                        }
                    }
                    Ok(Event::Eof) => break,
                    _ => (),
                }
            }
        }
        false
    }

    fn write_menu_file(&self, content: &[u8]) -> Result<(), MenuInstError> {
        tracing::info!("Writing {}", self.menu_config_location.display());
        let mut file = File::create(&self.menu_config_location)?;
        file.write_all(content)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn ensure_menu_file(&self) -> Result<(), MenuInstError> {
        if self.menu_config_location.exists() && !self.menu_config_location.is_file() {
            panic!(
                "Menu config location {} is not a file!",
                self.menu_config_location.display()
            );
            // return Err(anyhow!("Menu config location {} is not a file!",
            //     self.menu_config_location.display()));
        }

        if self.menu_config_location.is_file() {
            let timestamp = Utc::now().format("%Y-%m-%d_%Hh%Mm%S").to_string();
            let backup = format!("{}.{}", self.menu_config_location.display(), timestamp);
            fs::copy(&self.menu_config_location, &backup)?;

            if !self.is_valid_menu_file() {
                fs::remove_file(&self.menu_config_location)?;
            }
        }

        if !self.menu_config_location.exists() {
            self.new_menu_file()?;
        }
        Ok(())
    }

    fn new_menu_file(&self) -> Result<(), MenuInstError> {
        tracing::info!("Creating {}", self.menu_config_location.display());
        let mut content = String::from("<!DOCTYPE Menu PUBLIC \"-//freedesktop//DTD Menu 1.0//EN\" \"http://standards.freedesktop.org/menu-spec/menu-1.0.dtd\">\n");
        content.push_str("<Menu>\n  <Name>Applications</Name>\n");

        if self.mode == "user" {
            content.push_str(&format!(
                "  <MergeFile type=\"parent\">{}</MergeFile>\n",
                self.system_menu_config_location.display()
            ));
        }
        content.push_str("</Menu>\n");
        fs::write(&self.menu_config_location, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test::test_data;

    use super::*;
    use tempfile::TempDir;

    fn setup_test_dir() -> (TempDir, MenuXml) {
        let temp_dir = TempDir::new().unwrap();
        let menu_config = temp_dir.path().join("applications.menu");
        let system_menu_config = temp_dir.path().join("system_applications.menu");

        let menu_xml = MenuXml::new(
            menu_config,
            system_menu_config,
            "Test Menu".to_string(),
            "user".to_string(),
        )
        .unwrap();

        (temp_dir, menu_xml)
    }

    #[test]
    fn test_new_menu_file() {
        let (_temp_dir, menu_xml) = setup_test_dir();
        menu_xml.new_menu_file().unwrap();
        assert!(menu_xml.is_valid_menu_file());
    }

    #[test]
    fn test_add_and_remove_menu() {
        let (_temp_dir, menu_xml) = setup_test_dir();
        menu_xml.new_menu_file().unwrap();

        let system_menu_location = menu_xml.system_menu_config_location.display().to_string();

        let res = fs::read_to_string(&menu_xml.menu_config_location).unwrap();
        let res = res.replace(&system_menu_location, "/path/to/system_menu");
        insta::assert_snapshot!(res);

        // Add menu
        menu_xml.add_menu().unwrap();
        assert!(menu_xml.has_menu().unwrap());

        let res = fs::read_to_string(&menu_xml.menu_config_location).unwrap();
        let res = res.replace(&system_menu_location, "/path/to/system_menu");
        insta::assert_snapshot!(res);

        // Remove menu
        menu_xml.remove_menu().unwrap();

        let res = fs::read_to_string(&menu_xml.menu_config_location).unwrap();
        let res = res.replace(&system_menu_location, "/path/to/system_menu");
        insta::assert_snapshot!(res);
        assert!(!menu_xml.has_menu().unwrap());
    }

    #[test]
    fn test_invalid_menu_file() {
        let (_temp_dir, menu_xml) = setup_test_dir();

        // Write invalid content
        fs::write(&menu_xml.menu_config_location, "<Invalid>XML</Invalid>").unwrap();
        assert!(!menu_xml.is_valid_menu_file());
    }

    #[test]
    fn test_ensure_menu_file() {
        let (_temp_dir, menu_xml) = setup_test_dir();

        // Test with non-existent file
        menu_xml.ensure_menu_file().unwrap();
        assert!(menu_xml.menu_config_location.exists());
        assert!(menu_xml.is_valid_menu_file());

        // Test with invalid file
        fs::write(&menu_xml.menu_config_location, "<Invalid>XML</Invalid>").unwrap();
        menu_xml.ensure_menu_file().unwrap();
        assert!(menu_xml.is_valid_menu_file());
    }

    #[test]
    fn test_remove_menu_xml_structure() {
        let (_temp_dir, menu_xml) = setup_test_dir();

        // Create initial menu file with content
        let initial_content = r#"<!DOCTYPE Menu PUBLIC "-//freedesktop//DTD Menu 1.0//EN"
     "http://standards.freedesktop.org/menu-spec/menu-1.0.dtd">
    <Menu>
        <Name>Applications</Name>
        <MergeFile type="parent">/path/to/system_menu</MergeFile>
        <Menu>
            <Name>Test Menu</Name>
            <Directory>test-menu.directory</Directory>
            <Include>
                <Category>Test Menu</Category>
            </Include>
        </Menu>
    </Menu>"#;

        fs::write(&menu_xml.menu_config_location, initial_content).unwrap();

        // Remove the menu
        menu_xml.remove_menu().unwrap();

        // Read and verify the result
        let result = fs::read_to_string(&menu_xml.menu_config_location).unwrap();

        insta::assert_snapshot!(result);
    }

    #[test]
    // load file from test data (example.menu) and add a new entry, then remove it
    fn test_add_and_remove_menu_xml_structure() {
        let (_temp_dir, menu_xml) = setup_test_dir();

        let test_data = test_data();
        let schema_path = test_data.join("linux-menu/example.menu");

        // Copy the example.menu file to the menu location
        fs::copy(&schema_path, &menu_xml.menu_config_location).unwrap();

        // Add the menu
        menu_xml.add_menu().unwrap();

        // Read and verify the result
        let result = fs::read_to_string(&menu_xml.menu_config_location).unwrap();
        insta::assert_snapshot!(result);

        // Remove the menu
        menu_xml.remove_menu().unwrap();

        // Read and verify the result
        let result = fs::read_to_string(&menu_xml.menu_config_location).unwrap();
        insta::assert_snapshot!(result);
    }
}
