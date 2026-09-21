//! `rattler skill`: print an agent skill (`SKILL.md`) that teaches coding
//! agents how to use the `rattler` CLI.
//!
//! The skill is a hybrid: a handwritten preamble with the guidance clap cannot
//! express (`skill_preamble.md`), followed by a compact command reference and
//! the per-command examples that are generated from the clap command tree at
//! runtime, so they can never drift from the actual CLI.

use std::{fmt::Write as _, path::PathBuf};

use clap::CommandFactory;
use miette::{Context, IntoDiagnostic};

use crate::Opt as CommandArgs;

/// The handwritten part of the skill. `{version}` is replaced at runtime.
const PREAMBLE: &str = include_str!("skill_preamble.md");

/// The skill directory name, which the agent skills specification requires to
/// match the `name` in the frontmatter.
const SKILL_NAME: &str = "rattler";

/// Subcommands that are not useful for an agent and are left out.
const SKIPPED_COMMANDS: &[&str] = &["help", "completion", "skill"];

/// Print an agent skill (SKILL.md) describing how to use the `rattler` CLI.
///
/// The output follows the agent skills specification (agentskills.io) and can
/// be installed into any coding agent that supports skills, for example with
/// `rattler skill --output .claude/skills`.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler skill                                # print the skill to stdout
  rattler skill --output .claude/skills        # write .claude/skills/rattler/SKILL.md"#)]
pub struct Opt {
    /// Directory to write the skill into as `<DIR>/rattler/SKILL.md` instead
    /// of printing it to stdout.
    #[clap(short, long, value_name = "DIR")]
    output: Option<PathBuf>,
}

pub fn skill(opt: Opt) -> miette::Result<()> {
    let skill = render();
    match opt.output {
        Some(dir) => {
            let dir = dir.join(SKILL_NAME);
            let path = dir.join("SKILL.md");
            std::fs::create_dir_all(&dir)
                .into_diagnostic()
                .wrap_err("failed to create the skill directory")?;
            std::fs::write(&path, skill)
                .into_diagnostic()
                .wrap_err("failed to write the skill")?;
            eprintln!("Wrote {}", path.display());
        }
        None => print!("{skill}"),
    }
    Ok(())
}

/// Renders the complete skill document.
fn render() -> String {
    let mut cmd = CommandArgs::command().name(SKILL_NAME).bin_name(SKILL_NAME);
    cmd.build();

    let mut out = PREAMBLE.replace("{version}", cmd.get_version().unwrap_or("unknown"));

    let commands = collect_commands(&cmd);

    out.push_str("\n## Commands\n\n");
    out.push_str("| Command | Description |\n|---|---|\n");
    for (path, sub) in &commands {
        let about = sub
            .get_about()
            .map(|about| about.to_string().replace('|', "\\|"))
            .unwrap_or_default();
        writeln!(out, "| `{path}` | {about} |").unwrap();
    }

    out.push_str("\n## Global options\n\n");
    for arg in cmd.get_arguments().filter(|arg| arg.is_global_set()) {
        let mut names = Vec::new();
        if let Some(short) = arg.get_short() {
            names.push(format!("-{short}"));
        }
        if let Some(long) = arg.get_long() {
            names.push(format!("--{long}"));
        }
        let help = arg.get_help().map(ToString::to_string).unwrap_or_default();
        writeln!(out, "- `{}`: {help}", names.join(", ")).unwrap();
    }

    out.push_str("\n## Examples\n");
    for (path, sub) in &commands {
        let Some(examples) = sub.get_after_help().or(sub.get_after_long_help()) else {
            continue;
        };
        // The `after_help` blocks are written as "Examples:\n  <command>  # <comment>".
        let text = examples.to_string();
        let body = text.trim_start_matches("Examples:").trim();
        writeln!(out, "\n### `{path}`\n\n```bash").unwrap();
        for line in body.lines() {
            writeln!(out, "{}", line.trim()).unwrap();
        }
        out.push_str("```\n");
    }

    out
}

/// Flattens the (nested) subcommands of `cmd` into `(path, command)` pairs in
/// definition order, e.g. `("rattler auth login", ...)`, leaving out hidden
/// commands and the ones in [`SKIPPED_COMMANDS`].
fn collect_commands(cmd: &clap::Command) -> Vec<(String, &clap::Command)> {
    fn walk<'a>(parent: &str, cmd: &'a clap::Command, out: &mut Vec<(String, &'a clap::Command)>) {
        for sub in cmd.get_subcommands() {
            if sub.is_hide_set() || SKIPPED_COMMANDS.contains(&sub.get_name()) {
                continue;
            }
            let path = format!("{parent} {}", sub.get_name());
            out.push((path.clone(), sub));
            walk(&path, sub, out);
        }
    }

    let mut out = Vec::new();
    walk(SKILL_NAME, cmd, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// The agent skills specification recommends keeping a `SKILL.md` under
    /// 500 lines because the whole file is loaded into the agent's context.
    #[test]
    fn test_skill_is_short() {
        let skill = render();
        let lines = skill.lines().count();
        assert!(lines < 500, "the skill is {lines} lines long");
    }

    #[test]
    fn test_skill_has_frontmatter() {
        let skill = render();
        let mut lines = skill.lines();
        assert_eq!(lines.next(), Some("---"));
        assert_eq!(lines.next(), Some("name: rattler"));
        assert!(lines.next().is_some_and(|l| l.starts_with("description: ")));
        assert_eq!(lines.next(), Some("---"));
        assert!(!skill.contains("{version}"));
    }

    /// Every user-facing command is listed, and the ones we leave out are not.
    #[test]
    fn test_skill_lists_all_commands() {
        let skill = render();
        for command in [
            "rattler solve",
            "rattler create",
            "rattler auth login",
            "rattler exec",
        ] {
            assert!(
                skill.contains(&format!("| `{command}` |")),
                "{command} missing"
            );
        }
        for command in SKIPPED_COMMANDS {
            assert!(!skill.contains(&format!("| `rattler {command}` |")));
        }
    }

    /// Every `rattler ...` example in the skill, handwritten or generated,
    /// must parse with the current CLI so the examples cannot drift.
    #[test]
    fn test_skill_examples_parse() {
        let skill = render();
        let mut in_code_block = false;
        for line in skill.lines() {
            if line.starts_with("```") {
                in_code_block = !in_code_block;
                continue;
            }
            if !in_code_block || !line.starts_with("rattler ") {
                continue;
            }
            let words = shlex::split(line).unwrap_or_else(|| panic!("cannot split {line:?}"));
            // Only the rattler invocation itself, not what it is piped into.
            let args = words
                .iter()
                .take_while(|word| !matches!(word.as_str(), "|" | ">" | ">>"))
                .map(String::as_str);
            CommandArgs::try_parse_from(args)
                .unwrap_or_else(|err| panic!("example {line:?} does not parse: {err}"));
        }
    }
}
