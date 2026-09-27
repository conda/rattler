#!/usr/bin/env nu

use std/assert
use check-breaking-changes.nu [classify-result clean-log clean-semver-target render-comment select-changed-crates workspace-dependency-impact]

let summary = "Summary semver requires new major version: 1 major and 0 minor checks failed"
assert equal (classify-result 0 "") "success"
assert equal (classify-result 1 $"details\n($summary)\n") "failure"
assert equal (classify-result 1 $"details\n($summary)\n    Finished [1.0s] crate\n") "failure"
assert equal (classify-result 1 "cargo failed") "error"
assert equal (classify-result 1 $"($summary)\ncargo failed\n") "error"
assert equal (clean-log "\u{1b}[31mred\u{1b}[0m") "red"

let rendered = (render-comment "removed public_fn\n```\n</details>")
let expected = ([
    "<!-- cargo-semver-checks-comment -->"
    "`cargo-semver-checks` detected API breaking changes compared with the pull request's base revision."
    ""
    "<details>"
    "<summary>Details</summary>"
    ""
    "    removed public_fn"
    "    ```"
    "    </details>"
    ""
    "</details>"
    ""
] | str join "\n")
assert equal $rendered $expected

let large_log = (0..10000 | each {|index| $"line ($index)" } | str join "\n")
let large_comment = (render-comment $large_log)
assert (($large_comment | encode utf-8 | bytes length) < 55000)
assert ($large_comment | str contains "    [earlier output truncated]")

let root = (pwd | path expand)
let current_metadata = {
    workspace_root: $root
    workspace_members: ["a", "private", "bin", "new", "proc"]
    packages: [
        {
            id: "a"
            name: "a"
            publish: null
            manifest_path: ($root | path join "crates" "a" "Cargo.toml")
            targets: [{kind: ["lib"]}]
            dependencies: [{name: "shared"}]
        }
        {
            id: "private"
            name: "private"
            publish: []
            manifest_path: ($root | path join "crates" "private" "Cargo.toml")
            targets: [{kind: ["lib"]}]
        }
        {
            id: "bin"
            name: "bin"
            publish: null
            manifest_path: ($root | path join "crates" "bin" "Cargo.toml")
            targets: [{kind: ["bin"]}]
        }
        {
            id: "new"
            name: "new"
            publish: null
            manifest_path: ($root | path join "crates" "new" "Cargo.toml")
            targets: [{kind: ["lib"]}]
        }
        {
            id: "proc"
            name: "proc"
            publish: ["crates-io"]
            manifest_path: ($root | path join "crates" "proc" "Cargo.toml")
            targets: [{kind: ["proc-macro"]}]
            dependencies: [{name: "other"}]
        }
    ]
}
let baseline_metadata = {
    packages: [
        {name: "a"}
        {name: "private"}
        {name: "bin"}
        {name: "proc"}
    ]
}

assert equal (
    select-changed-crates $current_metadata $baseline_metadata ["crates/a/src/lib.rs"]
) ["a"]
assert equal (
    select-changed-crates $current_metadata $baseline_metadata ["Cargo.lock"]
) []
assert equal (
    select-changed-crates $current_metadata $baseline_metadata ["Cargo.toml"] ["shared"]
) ["a"]
assert equal (
    select-changed-crates $current_metadata $baseline_metadata ["Cargo.toml"] [] true
) ["a", "proc"]
assert equal (
    select-changed-crates $current_metadata $baseline_metadata ["README.md"]
) []

let baseline_manifest = {
    workspace: {
        members: ["crates/*"]
        dependencies: {
            shared: {version: "1"}
            unchanged: "1"
        }
    }
}
let current_manifest = {
    workspace: {
        members: ["crates/*"]
        dependencies: {
            shared: {version: "2"}
            added: "1"
            unchanged: "1"
        }
    }
}
assert equal (
    workspace-dependency-impact $current_manifest $baseline_manifest
) {force_all: false, dependencies: ["added", "shared"]}
assert equal (
    workspace-dependency-impact ($current_manifest | upsert workspace.members ["crates/*", "tools/*"])
        $baseline_manifest
) {force_all: true, dependencies: []}

let target = (mktemp --directory)
let semver_target = ($target | path join "semver-checks")
let retained_file = ($target | path join "retained")
mkdir $semver_target
"temporary" | save ($semver_target | path join "artifact")
"retained" | save $retained_file
clean-semver-target $target
assert (not ($semver_target | path exists))
assert ($retained_file | path exists)
rm --recursive --force $target

print "breaking change script tests passed"
