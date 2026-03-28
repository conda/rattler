#!/usr/bin/env nu

let py = (open pyproject.toml | get project.version)
let cargo = (open Cargo.toml | get package.version)

if $py == $cargo {
  exit 0
}

print --stderr $"py-rattler/pyproject.toml has ($py), py-rattler/Cargo.toml has ($cargo)"
exit 1
