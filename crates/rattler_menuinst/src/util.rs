use std::{io::Write, os::unix::fs::PermissionsExt, process::Command};

use fs_err as fs;

use crate::MenuInstError;

pub fn run_pre_create_command(pre_create_command: &str) -> Result<(), MenuInstError> {
    let mut temp_file = tempfile::NamedTempFile::with_suffix(".sh")?;
    temp_file.write_all(pre_create_command.as_bytes())?;
    let temp_path = temp_file.into_temp_path();

    // Mark the file as executable or run it with bash
    let mut cmd = if pre_create_command.starts_with("!#") {
        fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755))?;
        Command::new(&temp_path)
    } else {
        let mut cmd = Command::new("bash");
        cmd.arg(&temp_path);
        cmd
    };

    let output = cmd.output()?;
    if !output.status.success() {
        tracing::error!(
            "Failed to run pre-create command (status: {}): \nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        return Err(MenuInstError::InstallError(format!(
            "Failed to run pre-create command: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}
