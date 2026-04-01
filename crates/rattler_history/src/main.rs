//! A small playground binary for experimenting with the `rattler_history` crate.
//!
//! Run with:
//!
//! ```text
//! pixi run -- cargo run -p rattler_history --bin history-playground -- <path-to-prefix>
//! ```
//!
//! If no path is given it falls back to `test-data/history/test.history`
//! (relative to the workspace root) for a quick smoke-test.

use std::{collections::HashSet, path::PathBuf};

use rattler_history::{parse_str, History, HistoryCommentLine, SpecsComment};

fn main() {
    let arg = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/history/test.history"),
        PathBuf::from,
    );

    // Accept either a conda prefix directory or a direct path to a history file.
    let history_path = if arg.is_dir() {
        arg.join("conda-meta").join("history")
    } else {
        arg
    };

    println!("Reading history from: {}", history_path.display());
    println!();

    let content = std::fs::read_to_string(&history_path).unwrap_or_else(|e| {
        eprintln!("error reading {}: {e}", history_path.display());
        std::process::exit(1);
    });

    let entries = parse_str(&content).unwrap_or_else(|e| {
        eprintln!("parse error: {e}");
        std::process::exit(1);
    });

    // ── 1. Raw parsed entries ─────────────────────────────────────────────
    println!("=== Parsed entries ({} total) ===", entries.len());
    for (i, entry) in entries.iter().enumerate() {
        println!(
            "  [{i:>2}] {} | {} packages | {} comments | diff={}",
            entry.date.format("%Y-%m-%d %H:%M:%S"),
            entry.packages.len(),
            entry.comments.len(),
            entry.is_diff(),
        );
    }
    println!();

    // ── 2. Reconstructed latest environment state ─────────────────────────
    let mut current = HashSet::<String>::new();
    for entry in &entries {
        if entry.is_diff() {
            for pkg in &entry.packages {
                if let Some(dist) = pkg.strip_prefix('+') {
                    current.insert(dist.to_owned());
                } else if let Some(dist) = pkg.strip_prefix('-') {
                    current.remove(dist);
                }
            }
        } else {
            current = entry.packages.clone();
        }
    }

    println!("=== Latest environment state ({} packages) ===", current.len());
    let mut sorted: Vec<&str> = current.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    for pkg in sorted.iter().take(20) {
        println!("  {pkg}");
    }
    if sorted.len() > 20 {
        println!("  ... ({} more)", sorted.len() - 20);
    }
    println!();

    // ── 3. User requests ──────────────────────────────────────────────────
    println!("=== User requests ===");
    let mut request_count = 0;
    for entry in &entries {
        let Some(cmd) = entry.cmd() else { continue };

        request_count += 1;
        println!("  [{}]", entry.date.format("%Y-%m-%d %H:%M:%S"));
        println!("    cmd:           {}", cmd.join(" "));

        if let Some(v) = entry.conda_version() {
            println!("    conda version: {v}");
        }

        for specs_comment in entry.specs_comments() {
            match specs_comment {
                SpecsComment::Update(specs) => {
                    println!("    update specs:  {}", specs.join(", "));
                }
                SpecsComment::Remove(specs) => {
                    println!("    remove specs:  {}", specs.join(", "));
                }
                SpecsComment::Neutered(specs) => {
                    println!("    neutered:      {}", specs.join(", "));
                }
                SpecsComment::Other { action, specs } => {
                    println!("    {action} specs: {}", specs.join(", "));
                }
            }
        }
        println!();
    }
    if request_count == 0 {
        println!("  (no user requests found)");
    }

    // ── 4. Smoke-test write_head ──────────────────────────────────────────
    println!("=== write_head smoke-test ===");
    let mut buf = Vec::<u8>::new();
    History::write_head(&mut buf, "2026-01-01 00:00:00", &["conda", "install", "numpy"]).unwrap();
    println!("  output:");
    for line in String::from_utf8(buf).unwrap().lines() {
        println!("    {line}");
    }

    // ── 5. Round-trip parse of synthetic input ────────────────────────────
    let round_trip = parse_str(
        "==> 2026-01-01 00:00:00 <==\n\
         # cmd: conda install numpy\n\
         +defaults::numpy-1.24.0-py311_0\n\
         # update specs: ['numpy']\n",
    )
    .unwrap();

    println!("=== Round-trip parse of synthetic input ===");
    for entry in &round_trip {
        println!("  date:     {}", entry.date.format("%Y-%m-%d %H:%M:%S"));
        println!("  is_diff:  {}", entry.is_diff());
        for comment in &entry.comments {
            match comment {
                HistoryCommentLine::Cmd(argv) => println!("  cmd:      {}", argv.join(" ")),
                HistoryCommentLine::CondaVersion(v) => println!("  version:  {v}"),
                HistoryCommentLine::Specs(s) => println!("  specs:    {s:?}"),
                HistoryCommentLine::Other(s) => println!("  other:    {s}"),
            }
        }
    }
}
