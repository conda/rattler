#!/usr/bin/env nu

# Validate a semver-check artifact and update its pull request comment.
# This script must run from the trusted default-branch checkout because the
# artifact was produced while compiling pull-request-controlled code.

use check-breaking-changes.nu [render-comment]

const comment_marker = "<!-- cargo-semver-checks-comment -->"
const comment_query = '.[] | select(.user.login == "github-actions[bot]" and (.body | contains("<!-- cargo-semver-checks-comment -->"))) | .id'
const sha_pattern = '^([0-9a-f]{40}|[0-9a-f]{64})$'

# Run gh and provide its stderr when an API request fails.
def gh-command [arguments: list<string>] {
    let result = (^gh ...$arguments | complete)
    if $result.exit_code != 0 {
        error make {
            msg: $"gh ($arguments | str join ' ') failed"
            help: ($result.stderr | str trim)
        }
    }
    $result.stdout
}

def read-field [artifact_dir: string, name: string] {
    let path = ($artifact_dir | path join $name)
    if not ($path | path exists) {
        error make {msg: $"Artifact field is missing: ($name)"}
    }
    open --raw $path | str trim
}

def validate-sha [sha: string] {
    if not ($sha =~ $sha_pattern) {
        error make {msg: $"Invalid git SHA: ($sha)"}
    }
}

# Fetch the current PR fields used both for artifact binding and stale checks.
def get-pr [repository: string, pr_number: string] {
    gh-command ["api", $"repos/($repository)/pulls/($pr_number)"] | from json
}

# workflow_run.pull_requests can be empty for forks. Bind the artifact PR to
# the trusted source repository and branch from the workflow run instead.
def validate-run-source [
    run_id: string
    pr_number: string
    pr: record
    run_head_repository: string
    run_head_branch: string
] {
    let current_head_repository = ($pr.head.repo.full_name? | default "")
    if $current_head_repository != $run_head_repository or $pr.head.ref != $run_head_branch {
        error make {msg: $"PR ($pr_number) is not associated with workflow run ($run_id)"}
    }
}

# pull_request.base.sha can lag behind the base branch. The first parent of
# GitHub's current synthetic merge commit is the exact base used by PR checks.
def current-base-sha [repository: string, pr: record] {
    let merge_sha = ($pr.merge_commit_sha? | default "")
    if ($merge_sha | is-empty) {
        return ""
    }
    let merge_commit = (gh-command ["api", $"repos/($repository)/commits/($merge_sha)"] | from json)
    if ($merge_commit.parents | is-empty) {
        ""
    } else {
        $merge_commit.parents | first | get sha
    }
}

def is-stale [pr: record, current_base_sha: string, head_sha: string, base_sha: string] {
    $pr.state != "open" or $pr.head.sha != $head_sha or $current_base_sha != $base_sha
}

def find-comments [repository: string, pr_number: string] {
    gh-command [
        "api"
        "--paginate"
        $"repos/($repository)/issues/($pr_number)/comments"
        "--jq"
        $comment_query
    ] | lines | where {|id| not ($id | is-empty) }
}

def delete-comment [repository: string, comment_id: string] {
    gh-command [
        "api"
        $"repos/($repository)/issues/comments/($comment_id)"
        "--method"
        "DELETE"
    ] | ignore
}

def update-failure-comment [repository: string, pr_number: string, body: string] {
    let comments = (find-comments $repository $pr_number)
    if ($comments | is-empty) {
        gh-command [
            "api"
            $"repos/($repository)/issues/($pr_number)/comments"
            "--method"
            "POST"
            "--raw-field"
            $"body=($body)"
        ] | ignore
    } else {
        gh-command [
            "api"
            $"repos/($repository)/issues/comments/($comments | first)"
            "--method"
            "PATCH"
            "--raw-field"
            $"body=($body)"
        ] | ignore
        $comments | skip 1 | each {|comment_id|
            delete-comment $repository $comment_id
        } | ignore
    }
}

def delete-resolved-comments [repository: string, pr_number: string] {
    find-comments $repository $pr_number | each {|comment_id|
        delete-comment $repository $comment_id
    } | ignore
}

def main [artifact_dir: string] {
    let repository = $env.REPOSITORY
    let run_id = $env.RUN_ID
    let run_conclusion = $env.RUN_CONCLUSION
    let run_head_sha = $env.RUN_HEAD_SHA
    let run_head_branch = $env.RUN_HEAD_BRANCH
    let run_head_repository = $env.RUN_HEAD_REPOSITORY

    let pr_number = (read-field $artifact_dir "pr_number")
    let run_sha = (read-field $artifact_dir "run_sha")
    let head_sha = (read-field $artifact_dir "head_sha")
    let base_sha = (read-field $artifact_dir "base_sha")
    mut result = (read-field $artifact_dir "result")

    if not ($pr_number =~ '^[0-9]+$') {
        error make {msg: $"Invalid PR number: ($pr_number)"}
    }
    validate-sha $run_sha
    validate-sha $head_sha
    validate-sha $base_sha
    if $run_sha != $run_head_sha and $head_sha != $run_head_sha {
        error make {msg: $"Artifact does not belong to workflow run ($run_id)"}
    }
    if not ($result in ["success", "failure", "error"]) {
        error make {msg: $"Invalid semver result: ($result)"}
    }

    # A failed origin workflow cannot create or resolve a warning, regardless
    # of what its untrusted artifact claims.
    if $run_conclusion != "success" {
        $result = "error"
    }

    let pr = (get-pr $repository $pr_number)
    (validate-run-source
        $run_id
        $pr_number
        $pr
        $run_head_repository
        $run_head_branch)

    let pr_base_sha = (current-base-sha $repository $pr)
    if (is-stale $pr $pr_base_sha $head_sha $base_sha) {
        print $"Ignoring stale result for PR ($pr_number)"
        return
    }
    if $result == "error" {
        print $"Preserving the existing comment because workflow run ($run_id) failed"
        return
    }

    # Re-fetch immediately before mutation. Combined with workflow concurrency,
    # this prevents an old run from changing a newer PR result.
    let current_pr = (get-pr $repository $pr_number)
    let current_base_sha = (current-base-sha $repository $current_pr)
    if (is-stale $current_pr $current_base_sha $head_sha $base_sha) {
        print $"Skipping stale result for PR ($pr_number)"
        return
    }

    if $result == "failure" {
        let logs = (open --raw ($artifact_dir | path join "logs"))
        let body = (render-comment $logs)
        $body | save --force ($artifact_dir | path join "comment.md")
        update-failure-comment $repository $pr_number $body
    } else {
        delete-resolved-comments $repository $pr_number
    }
}
