let py = (yq -oy '.project.version' pyproject.toml | str trim)
let cargo = (yq -oy '.package.version' Cargo.toml | str trim)

if $py == $cargo {
  exit 0
}

print --stderr 'pyproject.toml and Cargo.toml versions differ'
exit 1
