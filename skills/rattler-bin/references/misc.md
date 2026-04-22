# Miscellaneous

Utility commands that don't need their own section.

---

## `rattler virtual-packages`

Prints the virtual packages detected on the current system, one per line, in `name=version=build` form. Handy for debugging solver decisions or for building `--virtual-package` overrides for `rattler create`.

```
rattler virtual-packages
```

Typical entries include `__unix`, `__linux`, `__osx`, `__glibc`, `__cuda`, `__archspec`.

**Example:**

```bash
rattler virtual-packages
# __unix=0=0
# __osx=14.5=0
# __archspec=1=arm64
```

---

## `rattler completion`

Generates a shell completion script for the `rattler` command and prints it to stdout.

```
rattler completion -s <SHELL>
```

| Shell value | Target |
|-------------|--------|
| `bash` | Bash |
| `zsh` | Zsh |
| `fish` | Fish |
| `elvish` | Elvish |
| `nushell` | Nushell |
| `powershell` | PowerShell |

**Examples:**

```bash
# Install Bash completion for the current user
rattler completion -s bash > ~/.local/share/bash-completion/completions/rattler

# Fish
rattler completion -s fish > ~/.config/fish/completions/rattler.fish

# Zsh (place on a directory that's in $fpath)
rattler completion -s zsh > "${fpath[1]}/_rattler"
```
