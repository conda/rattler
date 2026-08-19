---
name: prepare-py-rattler-release
description: Prepare a py-rattler (Python bindings) release PR against conda/rattler — assess what landed since the last tag, pick the version bump, write the CHANGELOG entry with tested example snippets, and open a draft PR from the fork. Use when asked to prepare, cut, or draft a py-rattler release, or to bump the Python bindings version.
---

# Prepare a py-rattler release

Opens one draft PR against `conda/rattler:main` from the maintainer's fork: bumps the version, writes the changelog section, lists whatever still has to land first.

Python bindings only. The crates in `crates/` release themselves through release-plz (the `chore: release (#NNNN)` commits). Publishing is separate: once this merges, someone dispatches `.github/workflows/release-python.yml` with the version as `tag`, which builds the wheels, pushes to PyPI and tags `py-rattler-v<version>`.

## 1. Isolated checkout

`jj root` tells you which VCS you are in. Colocated repos take either.

Don't work in the checkout you already have open. It usually carries churn you don't want in a release commit.

```powershell
# jj
jj git fetch --remote upstream
jj workspace add --name release-py ..\rattler-release-py
jj new 'main@upstream'   # inside the new workspace

# git
git fetch upstream
git worktree add -b prepare-py-rattler-v<version> ..\rattler-release-py upstream/main
```

jj wants `main@upstream`, git wants `upstream/main`. `jj workspace add --revision 'upstream/main'` fails *after* creating the directory, so just `jj new` inside it.

A jj workspace has no `.git`, so run `git log`/`git show`/`gh` from the colocated checkout and use `jj diff`/`jj st` inside the workspace. A git worktree has one and everything runs in place.

Afterwards: `jj workspace forget release-py` or `git worktree remove ..\rattler-release-py`, then delete the directory.

`origin` is the fork, `upstream` is `conda/rattler`. Check `git remote -v` instead of guessing the owner.

## 2. Range

```powershell
git tag --list 'py-rattler-v*' --sort=-v:refname | Select-Object -First 1
git log --oneline <last-tag>..upstream/main
```

Read `## [Unreleased]` in `py-rattler/CHANGELOG.md` too. Earlier PRs may have filed entries there; fold them in rather than duplicating them.

## 3. Triage

py-rattler is a thin PyO3 wrapper and the wheel vendors the crates, so a crate change ships to Python users even when nothing under `py-rattler/` was touched. Ask "would a Python user notice this?", not "which directory did it touch?".

Drop `chore(ci)`, renovate and dependabot bumps, `chore: release`, js-rattler-only changes, test-only changes, docs-only changes. Keep new or changed Python API, bugs users actually hit, security fixes, real performance work, new platforms, and dependency bumps that change behavior.

Listing what each commit touched under `py-rattler/` separates API work from dependency noise quickly:

```powershell
$commits = git log --format='%h %s' <last-tag>..upstream/main -- py-rattler/
foreach ($c in $commits) {
  $h = $c.Split(' ')[0]
  $files = git show --pretty=format: --name-only $h -- py-rattler/ | Where-Object { $_ -and $_ -notmatch 'Cargo.lock|pixi.lock' }
  if ($files) { Write-Output "=== $c"; $files | ForEach-Object { Write-Output "    $_" } }
}
```

`py-rattler/rattler/**.py` or `py-rattler/src/**.rs` means the Python surface moved. Only `py-rattler/Cargo.toml` is usually a dependency bump.

Don't read dependency versions off commit subjects. Bumps ride along inside unrelated feature PRs, and a PR titled after its own feature can quietly move the solver two minor versions. Diff the lock across the range instead, and pay attention to anything the bindings actually run (`resolvo` above all, since every `solve()` is it):

```powershell
git show <last-tag>:py-rattler/Cargo.lock | Select-String -Pattern 'name = "resolvo"' -Context 0,1
git show upstream/main:py-rattler/Cargo.lock | Select-String -Pattern 'name = "resolvo"' -Context 0,1
```

The wheel ships whatever the lock says: `MATURIN_PEP517_ARGS` in `py-rattler/pixi.toml` includes `--locked`.

## 4. Breaking changes

Rust API breakage and py-rattler breakage are mostly unrelated. A renamed Rust type is invisible from Python unless the bindings expose it, and most of those shouldn't be in the changelog at all. The other direction matters more: a Rust change with no API break can still break Python users, because the surface stayed identical while the behavior under it moved. That is the case that gets missed.

Breaking means any of:

- removed or renamed symbol, changed signature, changed return type, changed default
- same call, different result. A different solve, a parser that now accepts or rejects, a changed `__str__`, a changed sort or cache key
- format changes: lockfile version, on-disk cache, repodata wire model

`feat!:` subjects and leftover `cargo-semver-checks` bot comments say where to look. They are not evidence.

**Ask about one PR at a time.** One candidate, one question, then wait. Don't batch them into a multi-select or a list to tick off. The maintainer usually wants to ask something back ("does that path even run without a gateway?") and a checkbox gives them nowhere to do it. Expect several turns.

Argue four things per candidate:

1. **The call.** `solve(...)`, `str(MatchSpec)`, `Gateway.query(...)`, `index_fs(...)`. Not "the solver changed". If you can't name one it isn't a candidate. Carry the name into the changelog entry too.
2. **The code path.** Python entry point → PyO3 binding → the crate item the PR changed, with file references so it can be checked. Say what makes a conditional path run. If you can't trace it end to end you don't understand it well enough to ask yet.
3. **Before → after**, with a real value. `foo py39h123_0` → `foo * py39h123_0`.
4. **Who it hits**, including when you think that's nobody.

Include the ones you're unsure about and say you're unsure. The verdict is the maintainer's: rejected means the entry loses its `**BREAKING:**` prefix, moves back to Added or Fixed, drops out of the highlights and stops counting toward the bump. Settle this before steps 5 and 6.

## 5. Version

py-rattler is pre-1.0, so breaking goes in the minor slot. Anything breaking gives `0.X+1.0`, otherwise `0.X.Y+1`. Say why before editing.

Three places have to agree, and `pixi run -e test lint` fails on the first two drifting (`scripts/ensure-version-synced.nu`):

- `py-rattler/Cargo.toml`, `[package] version`
- `py-rattler/pyproject.toml`, `[project] version`
- `py-rattler/Cargo.lock`, through `pixi run -- cargo update --manifest-path py-rattler/Cargo.toml --workspace --offline`. `cargo` is not on `PATH` outside pixi.

The lock diff should be that one version line; a wider refresh is fine but mention it. `rattler.__version__` comes from the crate version at runtime, so there's nothing to bump in the Python sources. `py-rattler/pixi.lock` should not change at all, and any `pixi run` inside `py-rattler/` rewrites it, so put it back with `jj restore --from '@-' py-rattler/pixi.lock` or `git checkout -- py-rattler/pixi.lock`.

## 6. Changelog

```markdown
## [Unreleased]

## [0.26.0] - 2026-08-19

### Highlights
### Added
### Changed
### Fixed
### Performance
```

- One line per entry, imperative, ending in `` in [#NNNN](https://github.com/conda/rattler/pull/NNNN) ``. Full links here, not bare `#NNNN`: this file renders on PyPI and the docs site.
- Breaking entries take `**BREAKING:** ` and come first under `### Changed`, whatever the commit type was. Name the old behavior and the new one so a reader can tell whether their code is affected.
- Every breaking change also gets a `**Breaking: ...**` paragraph in the highlights with the migration. Never let one show up only as a bullet.
- Highlights cover the few things a user would actually want to know: a new capability, a changed way of doing something, or a speedup big enough to feel. Add a short snippet where that beats prose, and drop the imports unless the import path is the surprising part. A measured performance win belongs here with its numbers, not buried at the bottom under `### Performance`.
- **Never hard-wrap.** One line per paragraph and per entry, however long. Nothing reflows markdown in this repo (dprint only does YAML), and re-wrapping one sentence rewrites the whole paragraph in the next diff. If you unwrap mechanically, watch the link-reference definitions at the bottom of the file and any YAML frontmatter; joining those silently breaks them. Reflow only your own section and check the diff shows nothing outside it.
- Omit empty sections. Security fixes link the advisory. Date with `Get-Date -Format 'yyyy-MM-dd'`.

## 7. Test the snippets

```powershell
pixi run -e test python -c "<snippet>"
```

from `py-rattler/`. The first run compiles the extension and takes 10+ minutes, so start it in the background right after step 2 and triage while it builds.

On Windows it will probably die in `aws-lc-sys` (rustls is the default feature): no NASM, and then `error C1083: Cannot open include file` from aws-lc's own C sources even with `cmake`, `ninja`, `nasm` and `AWS_LC_SYS_CMAKE_BUILDER = "1"` added to `py-rattler/pixi.toml`. That's the machine, not the release. Don't sink more than an attempt or two into it: use WSL or Linux, or check the snippets against `py-rattler/rattler/**` and say in the PR they weren't run. Revert any pixi.toml workaround. (`pixi exec nasm` doesn't help, rattler-build rebuilds `PATH`.)

Wrap async snippets in `asyncio.run(...)` to test them. Prefer constructions that work offline. Never present an untested snippet as tested.

## 8. Pending work

Open PRs labelled `python-bindings` are usually it:

```powershell
$prs = gh pr list --repo conda/rattler --label python-bindings --state open --limit 50 --json number,title,isDraft | ConvertFrom-Json
foreach ($p in $prs) { "#$($p.number) [$(if($p.isDraft){'draft'}else{'ready'})] $($p.title)" }
```

Build the strings in a loop; a `-q` jq expression with spaces gets mangled by PowerShell.

Show that list, ask whether any should land first and whether anything else is pending, then look up whatever gets named with `gh pr view`, `gh issue view` or `gh search prs`. Say so if you can't find something instead of guessing a number.

## 9. Open the PR

```powershell
# jj (no --allow-new; create the bookmark locally first)
jj describe -m 'chore: prepare py-rattler v<version> release'
jj commit
jj bookmark create prepare-py-rattler-v<version> --revision '@-'
jj git push --remote origin --bookmark prepare-py-rattler-v<version>

# git (the branch already exists from `worktree add -b`)
git add -A
git commit -m 'chore: prepare py-rattler v<version> release'
git push -u origin prepare-py-rattler-v<version>
```

```powershell
gh pr create --repo conda/rattler --base main --head <fork-owner>:prepare-py-rattler-v<version> --draft --title "chore: prepare py-rattler v<version> release" --body-file <file>
```

Amending later is `jj squash` then a plain `jj git push` (the bookmark follows the rewrite), or `git commit --amend` and `git push --force-with-lease`.

**Keep the body short.** Link the rendered changelog near the top, then write only what it doesn't say. Half a screen, and if a sentence is already true in the changelog, cut it rather than reword it.

```markdown
**[changelog](https://github.com/<fork-owner>/rattler/blob/<branch>/py-rattler/CHANGELOG.md)**
```

Include the version and the one or two changes that forced the bump level, the judgment calls from step 4 with an invitation to push back (that part exists nowhere else, and it's the only bit a reviewer can really check you on), the pending checklist under `### Pending before release`, whether the snippets ran, and the template's AI disclosure with `Tools:`. Don't paste the user's prompt. `AGENTS.md` suggests it, but on a release PR it's long and buries everything that matters. Leave the disclosure boxes for the maintainer to tick.

Reference PRs bare (`#2700`), never with a copied title. GitHub attaches the live title, a copied one goes stale the moment someone retitles. Where the reader needs to know what a PR does to them, describe the effect instead ("`solve()` follows CEP-42 relations by default"); that's content, not a title.

Reread the body before posting and rewrite whatever sounds like AI wrote it: spaced em dashes as an all-purpose connector, bolded lead-ins, every paragraph the same length, sentences built in neat parallel. Break it up until it reads like a person wrote it. Short sentences are fine, and saying a caveat bothers you beats dressing it up as a limitation.

Draft as long as anything is pending, and `gh pr ready <n> --repo conda/rattler` only once the maintainer confirms. If nothing is pending, still open as draft and ask.

When a pending PR lands the notes are stale. Rebase onto `main@upstream`, re-run steps 2 to 4 over the new commits, add entries for whatever API arrived, and say in the PR which items are already folded in.

## Checklist

- [ ] fresh worktree or workspace on `upstream/main`, not the checkout you had open
- [ ] whole range triaged, drops deliberate
- [ ] each breaking candidate argued with its call and traced code path, asked one at a time, confirmed
- [ ] bump level justified
- [ ] `Cargo.toml`, `pyproject.toml` and `Cargo.lock` agree, `pixi.lock` untouched
- [ ] changelog dated, grouped, linked, breaking marked and called out in the highlights
- [ ] snippets run, or reported as unverified
- [ ] `python-bindings` PRs checked and pending work listed
- [ ] PR body links the changelog, repeats none of it, humanized
- [ ] opened as a draft from the fork against `conda/rattler:main`
