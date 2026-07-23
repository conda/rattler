#!/usr/bin/env nu

# Check changed public crates for API breaking changes.
#
# Local usage (requires cargo-semver-checks 0.48 in PATH):
#   nu scripts/check-breaking-changes.nu --base-ref main
#   nu scripts/check-breaking-changes.nu render breaking-changes.log
#
# The check writes cargo-semver-checks output to `breaking-changes.log`. When a
# breaking change is found, both commands produce the exact PR comment body in
# `breaking-changes.md`.

const repo_root = path self ..
const public_target_kinds = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"]
const workspace_files = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"]
const comment_marker = "<!-- cargo-semver-checks-comment -->"
const breaking_summary = "semver requires new major version:"

# Run a command and turn a non-zero exit code into a readable Nushell error.
def run-command [description: string, command: closure] {
    let result = (do $command | complete)
    if $result.exit_code != 0 {
        let details = ($result.stderr | str trim)
        error make {
            msg: $"($description) failed"
            help: (if ($details | is-empty) { null } else { $details })
        }
    }
    $result.stdout
}

# Keep log artifacts bounded and free of terminal control sequences.
export def clean-log [log: string, --limit: int = 200000] {
    let cleaned = ($log
        | ansi strip
        | str replace --all "\r\n" "\n"
        | str replace --all "\r" "\n"
        | str replace --all --regex "[\\x00-\\x08\\x0b\\x0c\\x0e-\\x1f\\x7f]" "")
    let length = ($cleaned | str length)
    if $length > $limit {
        $cleaned | str substring (($length - $limit)..)
    } else {
        $cleaned
    }
}

# cargo-semver-checks uses a major-version summary for breaking changes. Other
# non-zero exits are tooling errors and must not be reported as API breakage.
export def classify-result [exit_code: int, stderr: string] {
    if $exit_code == 0 {
        return "success"
    }

    let lines = ($stderr
        | ansi strip
        | str replace --all "\r" "\n"
        | lines
        | where {|line| not ($line | str trim | is-empty) })
    let summaries = ($lines | enumerate | where {|row| $row.item =~ $breaking_summary })
    if ($summaries | is-empty) {
        return "error"
    }

    # cargo-semver-checks can print a final timing line after its summary. Any
    # other trailing output indicates that the command failed for another reason.
    let summary_index = ($summaries | last | get index)
    let trailing = ($lines | skip ($summary_index + 1))
    if ($trailing | all {|line| $line | str trim | str starts-with "Finished [" }) {
        "failure"
    } else {
        "error"
    }
}

# Select changed packages from cargo metadata. Keeping this pure makes package
# selection easy to test without invoking cargo or git.
export def select-changed-crates [
    current_metadata: record
    baseline_metadata: record
    changed_files: list<string>
] {
    let baseline_packages = ($baseline_metadata.packages | get name)
    let check_all = ($changed_files | any {|file|
        ($file in $workspace_files) or ($file | str starts-with ".cargo/")
    })

    $current_metadata.packages
        | where {|package| $package.id in $current_metadata.workspace_members }
        | where {|package| $package.publish != [] }
        | where {|package| $package.name in $baseline_packages }
        | where {|package|
            $package.targets | any {|target|
                $target.kind | any {|kind| $kind in $public_target_kinds }
            }
        }
        | insert directory {|package|
            $package.manifest_path
                | path dirname
                | path relative-to $current_metadata.workspace_root
                | str replace --all "\\" "/"
        }
        | where {|package|
            $check_all or ($changed_files | any {|file|
                $file | str starts-with $"($package.directory)/"
            })
        }
        | get name
        | sort
        | uniq
}

# Render the exact Markdown body used by the privileged comment workflow.
# Every untrusted log line is indented so backticks and HTML remain code.
export def render-comment [log: string] {
    let cleaned = (clean-log $log)
    let content = (if ($cleaned | str trim | is-empty) {
        "No detailed output was captured. See the workflow run for details."
    } else {
        $cleaned | str trim --right
    })

    let detail_lines = ($content | lines | each {|line| $"    ($line)" })
    mut selected_lines = []
    mut selected_bytes = 0
    mut truncated = false

    for line in ($detail_lines | reverse) {
        let line_bytes = ($"($line)\n" | encode utf-8 | bytes length)
        if ($selected_bytes + $line_bytes) > 48000 {
            $truncated = true
            break
        }
        $selected_lines = ($selected_lines | prepend $line)
        $selected_bytes += $line_bytes
    }

    if ($selected_lines | is-empty) {
        # One very long line still needs a safe, bounded representation. A
        # Unicode scalar is at most four UTF-8 bytes.
        let length = ($content | str length)
        let start = ([$length - 11000, 0] | math max)
        $selected_lines = [$"    ($content | str substring $start..)" ]
        $truncated = true
    }
    if $truncated {
        $selected_lines = ($selected_lines | prepend "    [earlier output truncated]")
    }

    let details = ($selected_lines | str join "\n")
    [
        $comment_marker
        "`cargo-semver-checks` detected API breaking changes compared with the pull request's base revision."
        ""
        "<details>"
        "<summary>Details</summary>"
        ""
        $details
        ""
        "</details>"
        ""
    ] | str join "\n"
}

# Include committed, staged, unstaged, and untracked files so the same command
# is useful while developing a change locally.
def changed-files [base_ref: string] {
    let committed = (run-command "reading committed changes" {
        ^git -C $repo_root diff --name-only $"($base_ref)...HEAD"
    } | lines)
    let working = (run-command "reading working tree changes" {
        ^git -C $repo_root diff --name-only HEAD
    } | lines)
    let untracked = (run-command "reading untracked files" {
        ^git -C $repo_root ls-files --others --exclude-standard
    } | lines)
    $committed | append $working | append $untracked | where {|path| not ($path | is-empty) } | uniq
}

# Run cargo-semver-checks while streaming its output and retaining both streams
# for the PR comment.
def semver-check [packages: list<string>, baseline_root: string] {
    let package_args = ($packages | each {|package| ["--package", $package] } | flatten)
    let args = [
        "semver-checks"
        "check-release"
        "--manifest-path"
        ($repo_root | path join "Cargo.toml")
        "--baseline-root"
        $baseline_root
        "--release-type"
        "minor"
    ] | append $package_args

    print $"Checking API compatibility for: ($packages | str join ', ')"
    let command_result = (with-env {NO_COLOR: "1", CARGO_TERM_COLOR: "never"} {
        ^cargo ...$args | complete
    })
    print --no-newline $command_result.stdout
    print --stderr --no-newline $command_result.stderr
    {
        result: (classify-result $command_result.exit_code $command_result.stderr)
        logs: (clean-log $"($command_result.stdout)($command_result.stderr)")
        exit_code: $command_result.exit_code
    }
}

# Compare the current checkout with a complete baseline worktree. A worktree is
# used instead of --baseline-rev because cargo-semver-checks does not extract git
# submodules, while rattler needs the libsolv submodule.
def check-ref [base_ref: string] {
    let scratch = (mktemp --directory)
    let baseline_root = ($scratch | path join "baseline")

    let attempt = (try {
        run-command "creating baseline worktree" {
            ^git -C $repo_root worktree add --detach $baseline_root $base_ref
        } | ignore
        run-command "initializing baseline submodules" {
            ^git -C $baseline_root submodule update --init --recursive
        } | ignore

        let current_metadata = (run-command "reading current cargo metadata" {
            ^cargo metadata --manifest-path ($repo_root | path join "Cargo.toml") --no-deps --format-version 1
        } | from json)
        let baseline_metadata = (run-command "reading baseline cargo metadata" {
            ^cargo metadata --manifest-path ($baseline_root | path join "Cargo.toml") --no-deps --format-version 1
        } | from json)
        let packages = (select-changed-crates $current_metadata $baseline_metadata (changed-files $base_ref))

        let outcome = (if ($packages | is-empty) {
            {result: "success", logs: "", exit_code: 0, packages: []}
        } else {
            let check = (semver-check $packages $baseline_root)
            $check | insert packages $packages
        })
        {ok: true, outcome: $outcome}
    } catch {|error|
        {ok: false, error: $error}
    })

    do { ^git -C $repo_root worktree remove --force $baseline_root } | complete | ignore
    if ($scratch | path exists) {
        rm --recursive --force $scratch
    }
    if not $attempt.ok {
        error make {
            msg: $attempt.error.msg
            help: ($attempt.error.help? | default null)
        }
    }
    $attempt.outcome
}

def resolve-ref [reference: string] {
    run-command $"resolving git reference ($reference)" {
        ^git -C $repo_root rev-parse $reference
    } | str trim
}

# Write the artifact consumed by the privileged workflow_run workflow.
def write-artifact [directory: string, outcome: record, metadata: record] {
    mkdir $directory
    $metadata.pr_number | save --force ($directory | path join "pr_number")
    $metadata.run_sha | save --force ($directory | path join "run_sha")
    $metadata.head_sha | save --force ($directory | path join "head_sha")
    $metadata.base_sha | save --force ($directory | path join "base_sha")
    $outcome.result | save --force ($directory | path join "result")
    $outcome.logs | save --force ($directory | path join "logs")
    ($outcome.packages | str join " ") | save --force ($directory | path join "packages")
    if $outcome.result == "failure" {
        render-comment $outcome.logs | save --force ($directory | path join "comment.md")
    }
}

# Run from GitHub Actions. Breaking changes are a successful workflow outcome;
# infrastructure and cargo errors remain failed checks.
def "main ci" [base_ref: string, artifact_dir: string] {
    let metadata = {
        pr_number: $env.PR_NUMBER
        run_sha: $env.RUN_SHA
        head_sha: $env.HEAD_SHA
        base_sha: (resolve-ref $base_ref)
    }

    try {
        let outcome = (check-ref $base_ref)
        write-artifact $artifact_dir $outcome $metadata
        if $outcome.result == "error" {
            exit 1
        }
    } catch {|error|
        let message = ([$error.msg, ($error.help? | default "")] | where {|part| not ($part | is-empty) } | str join "\n")
        print --stderr $message
        write-artifact $artifact_dir {
            result: "error"
            logs: (clean-log $message)
            exit_code: 1
            packages: []
        } $metadata
        exit 1
    }
}

# Render a saved cargo-semver-checks log without rerunning the check.
def "main render" [
    log_file: string
    --output: string = "breaking-changes.md"
] {
    render-comment (open --raw $log_file) | save --force $output
    print $"Comment written to ($output)."
}

# Local entry point. This uses the same check and Markdown renderer as CI.
def main [
    --base-ref: string = "main"
    --comment: string = "breaking-changes.md"
    --logs: string = "breaking-changes.log"
] {
    let outcome = (check-ref $base_ref)
    $outcome.logs | save --force $logs

    if $outcome.result == "failure" {
        render-comment $outcome.logs | save --force $comment
        print $"Breaking API changes found. Comment written to ($comment)."
        exit 1
    } else if $outcome.result == "error" {
        error make {msg: $"cargo-semver-checks failed. See ($logs)."}
    } else {
        print "No breaking API changes found."
    }
}
