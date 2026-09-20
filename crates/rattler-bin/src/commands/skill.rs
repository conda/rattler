//! PROTOTYPE: generate an agent skill (SKILL.md) from the clap command tree.
use std::fmt::Write as _;

use clap::CommandFactory;

use crate::Opt as CommandArgs;

/// Print an agent skill (SKILL.md) describing the `rattler` CLI.
#[derive(clap::Parser, Debug)]
pub struct Opt {}

pub fn skill(_opt: Opt) -> miette::Result<()> {
    let mut cmd = CommandArgs::command().name("rattler").bin_name("rattler");
    cmd.build();
    let mut out = String::new();

    // --- frontmatter (agentskills.io spec) ---
    let about = cmd
        .get_long_about()
        .or(cmd.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    writeln!(out, "---").unwrap();
    writeln!(out, "name: rattler").unwrap();
    writeln!(
        out,
        "description: \"Use the `rattler` CLI to solve, create, inspect and manipulate conda environments and packages. {about} Use when the user mentions rattler, conda packages, .conda/.tar.bz2 archives, repodata or conda channels.\""
    )
    .unwrap();
    writeln!(out, "---\n").unwrap();

    writeln!(
        out,
        "# rattler CLI (v{})\n",
        cmd.get_version().unwrap_or("?")
    )
    .unwrap();
    writeln!(
        out,
        "Run `rattler <command> --help` for the authoritative flags of the installed version.\n"
    )
    .unwrap();

    writeln!(out, "## Global options\n").unwrap();
    for arg in cmd.get_arguments().filter(|a| a.is_global_set()) {
        write_arg(&mut out, arg);
    }

    writeln!(out, "\n## Commands\n").unwrap();
    for sub in cmd.get_subcommands() {
        write_command(&mut out, "rattler", sub);
    }
    print!("{out}");
    Ok(())
}

fn write_arg(out: &mut String, arg: &clap::Arg) {
    let mut names = Vec::new();
    if let Some(s) = arg.get_short() {
        names.push(format!("-{s}"));
    }
    if let Some(l) = arg.get_long() {
        names.push(format!("--{l}"));
    }
    let mut name = if names.is_empty() {
        format!("<{}>", arg.get_id().to_string().to_uppercase())
    } else {
        names.join(", ")
    };
    if arg.get_action().takes_values() && !names.is_empty() {
        let vn = arg
            .get_value_names()
            .and_then(|v| v.first().map(ToString::to_string))
            .unwrap_or_else(|| arg.get_id().to_string().to_uppercase());
        name.push_str(&format!(" <{vn}>"));
    }
    let help = arg
        .get_long_help()
        .or(arg.get_help())
        .map(|h| h.to_string().replace('\n', " "))
        .unwrap_or_default();
    let mut extra = Vec::new();
    let possible: Vec<_> = arg
        .get_possible_values()
        .iter()
        .filter(|p| !p.is_hide_set())
        .map(|p| format!("`{}`", p.get_name()))
        .collect();
    if !possible.is_empty() {
        extra.push(format!("one of {}", possible.join(", ")));
    }
    let defaults: Vec<_> = arg
        .get_default_values()
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .collect();
    if !defaults.is_empty() {
        extra.push(format!("default `{}`", defaults.join(",")));
    }
    if let Some(env) = arg.get_env() {
        extra.push(format!("env `{}`", env.to_string_lossy()));
    }
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!(" ({})", extra.join("; "))
    };
    writeln!(out, "- `{name}` — {help}{extra}").unwrap();
}

/// Render one (sub)command and recurse into its nested subcommands.
fn write_command(out: &mut String, parent: &str, sub: &clap::Command) {
    // Skip commands that are useless to an agent.
    if sub.is_hide_set() || matches!(sub.get_name(), "help" | "completion" | "skill") {
        return;
    }
    let path = format!("{parent} {}", sub.get_name());
    let about = sub
        .get_long_about()
        .or(sub.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    writeln!(out, "### `{path}`\n").unwrap();
    writeln!(out, "{about}\n").unwrap();
    let mut usage = sub.clone();
    writeln!(out, "```\n{}\n```\n", usage.render_usage()).unwrap();
    let args: Vec<_> = sub
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_global_set() && a.get_id() != "help")
        .collect();
    if !args.is_empty() {
        for arg in args {
            write_arg(out, arg);
        }
        writeln!(out).unwrap();
    }
    if let Some(after) = sub.get_after_help().or(sub.get_after_long_help()) {
        // The existing `after_help` blocks are all "Examples:\n  ..." – reuse them.
        let text = after.to_string();
        let body = text.trim_start_matches("Examples:").trim();
        writeln!(out, "Examples:\n\n```bash").unwrap();
        for line in body.lines() {
            writeln!(out, "{}", line.trim()).unwrap();
        }
        writeln!(out, "```\n").unwrap();
    }
    for nested in sub.get_subcommands() {
        write_command(out, &path, nested);
    }
}
