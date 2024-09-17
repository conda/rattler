//! This should take a serde_json file, render it with all variables and then load it as a MenuInst struct

use rattler_conda_types::Platform;
use std::{collections::HashMap, path::Path};

use crate::schema::MenuInstSchema;

fn replace_placeholders(mut text: String, replacements: &HashMap<String, String>) -> String {
    for (key, value) in replacements {
        let placeholder = format!("{{{{ {} }}}}", key);
        text = text.replace(&placeholder, value);
    }
    text
}

//
pub fn placeholders(
    base_prefix: &Path,
    prefix: &Path,
    platform: &Platform,
    menu_item_location: &Path,
) -> HashMap<String, String> {
    let dist_name = |p: &Path| {
        p.parent()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "empty".to_string())
    };

    let (python, base_python) = if platform.is_windows() {
        (prefix.join("python.exe"), base_prefix.join("python.exe"))
    } else {
        (prefix.join("bin/python"), base_prefix.join("bin/python"))
    };

    let mut vars = HashMap::from([
        ("BASE_PREFIX", base_prefix.to_path_buf()),
        ("PREFIX", prefix.to_path_buf()),
        ("PYTHON", python),
        ("BASE_PYTHON", base_python),
        ("MENU_DIR", prefix.join("menu")),
        (
            "HOME",
            dirs::home_dir()
                .map(|p| p.to_path_buf())
                .unwrap_or_default(),
        ),
    ]);

    if platform.is_windows() {
        vars.insert("BIN_DIR", prefix.join("Library/bin"));
        vars.insert("SCRIPTS_DIR", prefix.join("Scripts"));
        vars.insert("BASE_PYTHONW", base_prefix.join("pythonw.exe"));
        vars.insert("PYTHONW", prefix.join("pythonw.exe"));
    } else {
        vars.insert("BIN_DIR", prefix.join("bin"));
    }

    if platform.is_osx() {
        vars.insert("PYTHONAPP", prefix.join("python.app/Contents/MacOS/python"));
    }

    vars.insert("MENU_ITEM_LOCATION", menu_item_location.to_path_buf());

    let mut vars: HashMap<String, String> = vars
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string_lossy().to_string()))
        .collect();

    let icon_ext = if platform.is_windows() {
        "ico"
    } else if platform.is_osx() {
        "icns"
    } else {
        "png"
    };
    vars.insert("ICON_EXT".to_string(), icon_ext.to_string());

    vars.insert("DISTRIBUTION_NAME".to_string(), dist_name(prefix));
    vars.insert("ENV_NAME".to_string(), dist_name(prefix));

    // TODO: (missing!) PY_VER, SP_DIR

    vars
}

//
pub fn render(
    file: &Path,
    variables: &HashMap<String, String>,
) -> Result<MenuInstSchema, std::io::Error> {
    let text = std::fs::read_to_string(file)?;
    let text = replace_placeholders(text, variables);

    let menu_inst: MenuInstSchema = serde_json::from_str(&text)?;

    Ok(menu_inst)
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use crate::render::render;

    #[test]
    fn test_render_gnuradio() {
        let test_data = crate::test::test_data();
        let schema_path = test_data.join("gnuradio/gnuradio-grc.json");

        let placeholders = crate::render::placeholders(
            Path::new("/home/base_prefix"),
            Path::new("/home/prefix"),
            &rattler_conda_types::Platform::Linux64,
            Path::new("/menu_item_location"),
        );

        let schema = render(&schema_path, &placeholders).unwrap();
        insta::assert_debug_snapshot!(schema);
    }
}
