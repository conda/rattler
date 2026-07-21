#!/usr/bin/env nu

const py_rattler_dir = path self ..

let py = (open ($py_rattler_dir | path join pyproject.toml) | get project.version)
let cargo = (open ($py_rattler_dir | path join Cargo.toml) | get package.version)

if $py == $cargo {
  exit 0
}

print --stderr $"py-rattler/pyproject.toml has ($py), py-rattler/Cargo.toml has ($cargo)"
exit 1
