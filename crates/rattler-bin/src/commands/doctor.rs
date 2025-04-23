use clap::Parser;
use colored::Colorize;
use std::path::PathBuf;
use anyhow::Result;

/// Check your rattler installation and environment for common issues
#[derive(Parser, Debug)]
#[clap(about)]
pub struct DoctorCommand {
    /// Run all checks without interactive prompts
    #[clap(long)]
    non_interactive: bool,

    /// Only show problems, skip healthy checks
    #[clap(long)]
    only_problems: bool,

    /// Path to the environment to check
    #[clap(long)]
    env_path: Option<PathBuf>,
}

impl DoctorCommand {
    pub async fn run(&self) -> Result<()> {
        println!("{}", "🔍 Running Rattler Doctor...".bold());
        println!("Checking your system for potential problems...\n");

        // Run our diagnostic checks
        let mut has_problems = false;

        // 1. System Configuration Checks
        has_problems |= self.check_system_configuration().await?;

        // 2. Environment Health Checks
        has_problems |= self.check_environment_health().await?;

        // 3. Performance Checks
        has_problems |= self.check_performance().await?;

        // 4. Security Checks
        has_problems |= self.check_security().await?;

        if !has_problems {
            println!("\n{}", "✅ No problems found!".green().bold());
        }

        Ok(())
    }

    async fn check_system_configuration(&self) -> Result<bool> {
        println!("{}", "🖥️  Checking system configuration...".bold());
        let mut has_problems = false;

        // Check PATH settings
        if let Some(problem) = self.check_path_settings().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "PATH settings look good".green());
        }

        // Check for conflicting installations
        if let Some(problem) = self.check_conflicting_installations().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "No conflicting installations found".green());
        }

        // Check environment variables
        if let Some(problem) = self.check_environment_variables().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "Environment variables are properly set".green());
        }

        println!();
        Ok(has_problems)
    }

    async fn check_environment_health(&self) -> Result<bool> {
        println!("{}", "🧪 Checking environment health...".bold());
        let mut has_problems = false;

        // Check for broken packages
        if let Some(problem) = self.check_broken_packages().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "All packages are healthy".green());
        }

        // Check for duplicate packages
        if let Some(problem) = self.check_duplicate_packages().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "No duplicate packages found".green());
        }

        println!();
        Ok(has_problems)
    }

    async fn check_performance(&self) -> Result<bool> {
        println!("{}", "⚡ Checking performance configuration...".bold());
        let mut has_problems = false;

        // Check channel configuration
        if let Some(problem) = self.check_channel_config().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "Channel configuration is optimal".green());
        }

        // Check cache size
        if let Some(problem) = self.check_cache_size().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "Cache size is reasonable".green());
        }

        println!();
        Ok(has_problems)
    }

    async fn check_security(&self) -> Result<bool> {
        println!("{}", "🔒 Checking security...".bold());
        let mut has_problems = false;

        // Check for outdated packages with vulnerabilities
        if let Some(problem) = self.check_package_vulnerabilities().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "No known vulnerabilities found".green());
        }

        // Check package signatures
        if let Some(problem) = self.check_package_signatures().await? {
            has_problems = true;
            println!("❌ {}", problem.red());
        } else if !self.only_problems {
            println!("✅ {}", "All package signatures are valid".green());
        }

        println!();
        Ok(has_problems)
    }

    // Individual check implementations
    async fn check_path_settings(&self) -> Result<Option<String>> {
        let path = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("PATH environment variable not found"))?;
        let paths: Vec<_> = std::env::split_paths(&path).collect();
        let mut problems = Vec::new();

        // Check for duplicate entries
        let mut seen = std::collections::HashSet::new();
        for path in &paths {
            if !seen.insert(path) {
                problems.push(format!("Duplicate PATH entry found: {}", path.display()));
            }
        }

        // Check for non-existent directories
        for path in &paths {
            if !path.exists() {
                problems.push(format!("Non-existent PATH entry: {}", path.display()));
            }
        }

        // Check for conda/rattler paths order
        let conda_paths: Vec<_> = paths.iter()
            .filter(|p| p.to_string_lossy().contains("conda") || p.to_string_lossy().contains("rattler"))
            .collect();

        if !conda_paths.is_empty() {
            // Check if conda paths are at the start of PATH
            let first_conda_index = paths.iter()
                .position(|p| p.to_string_lossy().contains("conda") || p.to_string_lossy().contains("rattler"))
                .unwrap_or(0);

            if first_conda_index > 0 {
                problems.push("Conda/Rattler paths should be at the start of your PATH to avoid conflicts".to_string());
            }

            // Check for multiple conda installations
            if conda_paths.len() > 1 {
                problems.push(format!(
                    "Multiple Conda/Rattler installations found in PATH: {}",
                    conda_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
                ));
            }
        }

        if problems.is_empty() {
            Ok(None)
        } else {
            Ok(Some(problems.join("\n")))
        }
    }

    async fn check_conflicting_installations(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_environment_variables(&self) -> Result<Option<String>> {
        let mut problems = Vec::new();

        // Important environment variables to check
        let critical_vars = [
            "CONDA_PREFIX",
            "CONDA_DEFAULT_ENV",
            "CONDA_SHLVL",
            "CONDA_PROMPT_MODIFIER",
        ];

        // Check if any critical variables are missing
        for var in critical_vars {
            if std::env::var(var).is_err() {
                problems.push(format!("Missing environment variable: {var}"));
            }
        }

        // Check CONDA_PREFIX matches actual environment path if specified
        if let Some(env_path) = &self.env_path {
            if let Ok(conda_prefix) = std::env::var("CONDA_PREFIX") {
                let conda_prefix = PathBuf::from(conda_prefix);
                if conda_prefix != env_path.canonicalize()? {
                    problems.push(format!(
                        "CONDA_PREFIX ({}) does not match specified environment path ({})",
                        conda_prefix.display(),
                        env_path.display()
                    ));
                }
            }
        }

        // Check CONDA_SHLVL is a valid number
        if let Ok(shlvl) = std::env::var("CONDA_SHLVL") {
            if shlvl.parse::<u32>().is_err() {
                problems.push("CONDA_SHLVL is not a valid number".to_string());
            }
        }

        // Check for old/deprecated variables that might cause issues
        let deprecated_vars = [
            "CONDA_ROOT",  // Deprecated in favor of CONDA_PREFIX
            "CONDA_ENVS_PATH",  // Deprecated in favor of CONDA_ENVS_DIRS
        ];

        for var in deprecated_vars {
            if std::env::var(var).is_ok() {
                problems.push(format!("Deprecated environment variable in use: {var}"));
            }
        }

        // Check for potential conflicting Python-related variables
        let python_vars = [
            ("PYTHONHOME", "might conflict with Conda Python"),
            ("PYTHONPATH", "might interfere with Conda environments"),
        ];

        for (var, message) in python_vars {
            if std::env::var(var).is_ok() {
                problems.push(format!("Warning: {var} is set - {message}"));
            }
        }

        // Check if we're in a conda environment but CONDA_PREFIX is not set
        let in_conda_env = std::env::var("CONDA_DEFAULT_ENV").is_ok();
        let conda_prefix_set = std::env::var("CONDA_PREFIX").is_ok();
        if in_conda_env && !conda_prefix_set {
            problems.push("In a Conda environment but CONDA_PREFIX is not set".to_string());
        }

        if problems.is_empty() {
            Ok(None)
        } else {
            Ok(Some(problems.join("\n")))
        }
    }

    async fn check_broken_packages(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_duplicate_packages(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_channel_config(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_cache_size(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_package_vulnerabilities(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }

    async fn check_package_signatures(&self) -> Result<Option<String>> {
        // TODO: Implement actual check
        Ok(None)
    }
} 