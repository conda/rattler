use rattler_shell::shell::{Shell, ShellEnum};
use std::path::PathBuf;

fn main() {
    // Get the current shell
    let current_shell = Shell::from_env().unwrap();

    // Generate activation script
    let env_path = PathBuf::from("/path/to/env");
    let script = current_shell.generate_activate_script(&env_path);
    println!("Activation script: {}", script);

    // Get shell-specific path separator
    let path_sep = current_shell.path_sep();
    println!("Path separator: {}", path_sep);
} 