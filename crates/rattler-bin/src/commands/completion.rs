use std::io::Write;

use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{shells, Generator};
use clap_complete_nushell::Nushell;
use miette::IntoDiagnostic;

use crate::Opt as CommandArgs;

/// Generates a completion script for a shell.
#[derive(Parser, Debug)]
pub struct Opt {
    /// The shell to generate a completion script for
    #[arg(short, long)]
    shell: Shell,
}

/// Defines the shells for which we can provide completions.
#[allow(clippy::enum_variant_names)]
#[derive(ValueEnum, Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Shell {
    /// Bash shell
    Bash,
    /// Elvish shell
    Elvish,
    /// Fish shell
    Fish,
    /// Nushell
    Nushell,
    /// `PowerShell` shell
    Powershell,
    /// Zsh shell
    Zsh,
}

impl Generator for Shell {
    fn file_name(&self, name: &str) -> String {
        match self {
            Self::Bash => shells::Bash.file_name(name),
            Self::Elvish => shells::Elvish.file_name(name),
            Self::Fish => shells::Fish.file_name(name),
            Self::Nushell => Nushell.file_name(name),
            Self::Powershell => shells::PowerShell.file_name(name),
            Self::Zsh => shells::Zsh.file_name(name),
        }
    }

    fn generate(&self, cmd: &clap::Command, buf: &mut dyn Write) {
        match self {
            Self::Bash => shells::Bash.generate(cmd, buf),
            Self::Elvish => shells::Elvish.generate(cmd, buf),
            Self::Fish => shells::Fish.generate(cmd, buf),
            Self::Nushell => Nushell.generate(cmd, buf),
            Self::Powershell => shells::PowerShell.generate(cmd, buf),
            Self::Zsh => shells::Zsh.generate(cmd, buf),
        }
    }
}

/// Generate completions for the rattler CLI and print them to stdout.
pub fn completion(args: Opt) -> miette::Result<()> {
    let mut buf = Vec::new();
    clap_complete::generate(args.shell, &mut CommandArgs::command(), "rattler", &mut buf);

    std::io::stdout().write_all(&buf).into_diagnostic()?;
    Ok(())
}
