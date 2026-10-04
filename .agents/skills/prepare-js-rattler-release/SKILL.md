---
name: prepare-js-rattler-release
description: Prepare a js-rattler (JavaScript/WASM bindings, npm package @conda-org/rattler) release PR against conda/rattler — assess what landed since the last tag, pick the version bump, write the CHANGELOG entry with tested example snippets, and open a draft PR from the fork. Use when asked to prepare, cut, or draft a js-rattler release, or to bump the JS/npm bindings version.
---

# Prepare a js-rattler release

Opens one draft PR against `conda/rattler:main` from the maintainer's fork: bumps the version, writes the changelog section, lists whatever still has to land first.

JS bindings only. The crates in `crates/` release themselves through release-plz (the `chore: release (#NNNN)` commits), and py-rattler has its own skill (`prepare-py-rattler-release`). Publishing is separate: once this merges, someone dispatches `.github/workflows/release-js.yml` with the version as `tag`, which runs `pixi run pack`, publishes the tarball to npm and tags `js-rattler-v<version>`. That workflow fails its `validate-tag` job unless `tag` equals `version` in `js-rattler/package.json` on the released commit. Dispatching it with an empty `tag` is a dry run that only builds and packs.

## 1. Isolated checkout

`jj root` tells you which VCS you are in. Colocated repos take either.

Don't work in the checkout you already have open. It usually carries churn you don't want in a release commit.

```powershell
# jj
jj git fetch --remote upstream
jj workspace add --name release-js ..\rattler-release-js
jj new 'main@upstream'   # inside the new workspace

# git
git fetch upstream --tags
git worktree add -b prepare-js-rattler-v<version> ..\rattler-release-js upstream/main
```

jj wants `main@upstream`, git wants `upstream/main`. `jj workspace add --revision 'upstream/main'` fails *after* creating the directory, so just `jj new` inside it.

A jj workspace has no `.git`, so run `git log`/`git show`/`gh` from the colocated checkout and use `jj diff`/`jj st` inside the workspace. A git worktree has one and everything runs in place.

Afterwards: `jj workspace forget release-js` or `git worktree remove ..\rattler-release-js`, then delete the directory.

`origin` is the fork, `upstream` is `conda/rattler`. Check `git remote -v` instead of guessing the owner. Tags have to be fetched (`--tags`), shallow or fresh clones often don't have `js-rattler-v*`.

## 2. Range

```powershell
git tag --list 'js-rattler-v*' --sort=-v:refname | Select-Object -First 1
git log --oneline <last-tag>..upstream/main
```

```bash
git tag --list 'js-rattler-v*' --sort=-v:refname | head -1
```

Read `## [Unreleased]` in `js-rattler/CHANGELOG.md` too. Earlier PRs may have filed entries there; fold them in rather than duplicating them.

Start the build from step 7 in the background now. The release-mode wasm-pack build runs twice (web and nodejs targets) with LTO and takes a while.

## 3. Triage

js-rattler is a thin wasm-bindgen wrapper and the `.wasm` statically links the crates, so a crate change ships to JS users even when nothing under `js-rattler/` was touched. Ask "would a JS user notice this?", not "which directory did it touch?".

Not every crate is in the bundle, though. Only the ones in `js-rattler/Cargo.lock` without a `source =` line are, so a change to e.g. `rattler_shell`, `rattler_index` or `rattler_menuinst` cannot reach JS users. List them with:

```bash
awk '/^\[\[package\]\]/{if(n&&!s)print n; n="";s=0} /^name = /{gsub(/"/,"",$3); n=$3} /^source = /{s=1} END{if(n&&!s)print n}' js-rattler/Cargo.lock
```

At the time of writing that is `rattler_conda_types`, `rattler_conda_version`, `rattler_digest`, `rattler_repodata_gateway`, `rattler_solve`, `rattler_networking`, `rattler_cache`, `rattler_package_streaming`, `rattler_redaction`, `rattler_macros`, `file_url`, `coalesced_map` and `simple_spawn_blocking`, all under `crates/<name>`. Re-run it rather than trusting this list. Restrict the log to those paths plus the bindings:

```bash
git log --oneline <last-tag>..upstream/main -- js-rattler/ crates/rattler_conda_types crates/rattler_conda_version crates/rattler_digest crates/rattler_repodata_gateway crates/rattler_solve crates/rattler_networking crates/rattler_cache crates/rattler_package_streaming crates/rattler_redaction crates/rattler_macros crates/file_url crates/coalesced_map crates/simple_spawn_blocking
```

Even inside those crates, most code isn't reachable from the bindings: the WASM build has no filesystem installs, no shell activation, no package extraction to disk. Check what `js-rattler/crate/*.rs` actually calls before counting a crate change.

Drop `chore(ci)`, renovate and dependabot bumps (most JS-side ones only touch `devDependencies` in `package.json`, which never ship), `chore: release`, py-rattler-only changes, test-only changes, docs-only changes. Keep new or changed JS API, bugs users actually hit, security fixes, real performance or bundle size work, and dependency bumps that change behavior.

Listing what each commit touched under `js-rattler/` separates API work from dependency noise quickly:

```powershell
$commits = git log --format='%h %s' <last-tag>..upstream/main -- js-rattler/
foreach ($c in $commits) {
  $h = $c.Split(' ')[0]
  $files = git show --pretty=format: --name-only $h -- js-rattler/ | Where-Object { $_ -and $_ -notmatch 'Cargo.lock|pixi.lock|package-lock.json' }
  if ($files) { Write-Output "=== $c"; $files | ForEach-Object { Write-Output "    $_" } }
}
```

```bash
git log --format='%h %s' <last-tag>..upstream/main -- js-rattler/ | while read -r h s; do
  files=$(git show --pretty=format: --name-only "$h" -- js-rattler/ | grep -vE 'Cargo.lock|pixi.lock|package-lock.json')
  [ -n "$files" ] && printf '=== %s %s\n%s\n' "$h" "$s" "$(echo "$files" | sed 's/^/    /')"
done
```

`js-rattler/crate/**.rs`, `js-rattler/crate/*.d.ts` or `js-rattler/src/*.ts` (not `*.test.ts`) means the JS surface moved. A new type is only public once `src/index.ts` exports it; 0.4.0 shipped exactly such a fix for `Gateway`, so check that too. Only `js-rattler/Cargo.toml` or `package.json` is usually a dependency bump.

Don't read dependency versions off commit subjects. Bumps ride along inside unrelated feature PRs. Diff the lock across the range instead, and pay attention to anything the bindings actually run (`resolvo` above all, since every `simpleSolve()` is it):

```bash
git diff <last-tag> upstream/main -- js-rattler/Cargo.lock
git show <last-tag>:js-rattler/Cargo.lock | grep -A1 'name = "resolvo"'
git show upstream/main:js-rattler/Cargo.lock | grep -A1 'name = "resolvo"'
```

js-rattler has its own `Cargo.lock`, separate from the workspace one; it's the one that ships (CI builds with `--locked`).

## 4. Breaking changes

Rust API breakage and js-rattler breakage are mostly unrelated. A renamed Rust type is invisible from JS unless the bindings expose it. The other direction matters more: a Rust change with no API break can still break JS users, because the surface stayed identical while the behavior under it moved. That is the case that gets missed.

Breaking means any of:

- removed or renamed export, changed constructor or method signature, changed return type, changed default
- changed TypeScript types in `dist/index.d.ts`, including the hand-written ones in `js-rattler/crate/*.d.ts` (`PackageName`, `Platform`, `PackageRecord`, `NoArchType`, `ParseStrictness`). A narrower type is a compile error for TS users even when the runtime didn't change.
- same call, different result. A different solve, a parser that now accepts or rejects, a changed `toString()`, a changed error `code`, a different shape of the JSON records `Gateway.query` returns
- packaging changes: the `exports` map, the `dist/` file names, ESM vs CJS behavior, a required newer Node or browser feature

`feat!:` subjects and leftover `cargo-semver-checks` bot comments say where to look. They are not evidence.

**Ask about one PR at a time.** One candidate, one question, then wait. Don't batch them into a multi-select or a list to tick off. The maintainer usually wants to ask something back and a checkbox gives them nowhere to do it. Expect several turns.

Argue four things per candidate:

1. **The call.** `simpleSolve(...)`, `new VersionSpec(...)`, `gateway.query(...)`, `version.bumpMinor()`. Not "the solver changed". If you can't name one it isn't a candidate. Carry the name into the changelog entry too.
2. **The code path.** JS export in `src/*.ts` → wasm-bindgen binding in `js-rattler/crate/*.rs` → the crate item the PR changed, with file references so it can be checked. Say what makes a conditional path run. If you can't trace it end to end you don't understand it well enough to ask yet.
3. **Before → after**, with a real value, e.g. `new VersionSpec("1.2.*").toString()` printing `"1.2.*"` before and `"=1.2"` after (illustrative, use what the PR actually changed).
4. **Who it hits**, including when you think that's nobody.

Include the ones you're unsure about and say you're unsure. The verdict is the maintainer's: rejected means the entry loses its `**BREAKING:**` prefix, moves back to Added or Fixed, drops out of the highlights and stops counting toward the bump. Settle this before steps 5 and 6.

## 5. Version

js-rattler is pre-1.0, so breaking goes in the minor slot. Anything breaking gives `0.X+1.0`, otherwise `0.X.Y+1`. Say why before editing.

The version lives in two places, and nothing in CI checks they agree, so check yourself:

- `js-rattler/package.json`, `version`. This is the one the release workflow validates against.
- `js-rattler/package-lock.json`, the two `version` fields for the root package at the top.

`npm version <version> --no-git-tag-version` from `js-rattler/` updates both and nothing else (`npm` comes from the pixi env: `pixi run -- npm version ...`). Without `--no-git-tag-version` it commits and tags on its own, which you don't want. The `package-lock.json` diff should be exactly those two lines.

Don't touch `js-rattler/Cargo.toml`. Its `version` (`0.1.1`) is not published anywhere and has never followed the npm version. Bumping it would also churn `js-rattler/Cargo.lock`.

`js-rattler/pixi.lock` should not change. `pixi run` may rewrite it, so put it back with `jj restore --from '@-' js-rattler/pixi.lock` or `git checkout -- js-rattler/pixi.lock`.

## 6. Changelog

```markdown
## [Unreleased]

## [0.5.0] - 2026-10-04

### Highlights
### Added
### Changed
### Fixed
### Performance
```

- One line per entry, imperative, ending in `` in [#NNNN](https://github.com/conda/rattler/pull/NNNN) ``. Full links here, not bare `#NNNN`: this file is read on GitHub and from the npm package page.
- Breaking entries take `**BREAKING:** ` and come first under `### Changed`, whatever the commit type was. Name the old behavior and the new one so a reader can tell whether their code is affected.
- Every breaking change also gets a `**Breaking: ...**` paragraph in the highlights with the migration. Never let one show up only as a bullet.
- Highlights cover the few things a user would actually want to know: a new capability, a changed way of doing something, or a speedup or size reduction big enough to feel. Add a short JS snippet where that beats prose, and drop the imports unless the import path is the surprising part. Look at the 0.4.0 section for the tone. A measured performance or bundle size win belongs here with its numbers (`ls -l dist/js_rattler_bg.wasm` before and after), not buried under `### Performance`.
- **Never hard-wrap new text.** One line per paragraph and per entry, however long. Re-wrapping one sentence rewrites the whole paragraph in the next diff. The file is formatted with Prettier (`.prettierrc` leaves `proseWrap` at its default, which keeps your line breaks as written), so run `pixi run fmt` from `js-rattler/` afterwards and check the diff shows nothing outside your section. The existing header paragraph at the top is wrapped; leave it alone.
- Omit empty sections. Security fixes link the advisory. Date with `Get-Date -Format 'yyyy-MM-dd'` or `date +%F`.

## 7. Test the snippets

From `js-rattler/`, build once:

```bash
pixi run build
```

Then run each snippet against the built package. The package can import itself by name from inside `js-rattler/` (that's how `e2e/main.mjs` works), so snippets use the same import line a user would:

```bash
node --input-type=module -e "
import { Version, VersionSpec } from '@conda-org/rattler';
console.log(new VersionSpec('~=1.2.0').matches(new Version('1.2.3')));
"
```

Module mode allows top-level `await`, so async snippets need no wrapper. Run `node e2e/main.cjs` once too, which covers the CommonJS build. Prefer constructions that work offline; `Gateway` snippets need network access to the channel. Never present an untested snippet as tested.

If the build won't run on the machine (it needs the `wasm32-unknown-unknown` std and `clang` from the pixi env), check the snippets against `js-rattler/src/**` and `js-rattler/crate/**` and say in the PR they weren't run. Reverting `dist/`, `pkg/` and `types/` is not needed, they're gitignored.

## 8. Pending work

There's no JS label on conda/rattler, so filter open PRs by the files they touch:

```bash
gh pr list --repo conda/rattler --state open --limit 100 --json number,title,isDraft,files --jq '.[] | select(any(.files[]; .path | startswith("js-rattler/"))) | "#\(.number) [\(if .isDraft then "draft" else "ready" end)] \(.title)"'
```

```powershell
$prs = gh pr list --repo conda/rattler --state open --limit 100 --json number,title,isDraft,files | ConvertFrom-Json
foreach ($p in $prs) { if ($p.files.path -like 'js-rattler/*') { "#$($p.number) [$(if($p.isDraft){'draft'}else{'ready'})] $($p.title)" } }
```

The PowerShell variant builds the strings in a loop because a `-q` jq expression with spaces gets mangled there. Renovate PRs that only bump JS dev tooling can be left out of the list.

Show that list, ask whether any should land first and whether anything else is pending (a crate fix the bindings need, for instance, won't show up in this filter), then look up whatever gets named with `gh pr view`, `gh issue view` or `gh search prs`. Say so if you can't find something instead of guessing a number.

## 9. Open the PR

```powershell
# jj (no --allow-new; create the bookmark locally first)
jj describe -m 'chore: prepare js-rattler v<version> release'
jj commit
jj bookmark create prepare-js-rattler-v<version> --revision '@-'
jj git push --remote origin --bookmark prepare-js-rattler-v<version>

# git (the branch already exists from `worktree add -b`)
git add -A
git commit -m 'chore: prepare js-rattler v<version> release'
git push -u origin prepare-js-rattler-v<version>
```

```powershell
gh pr create --repo conda/rattler --base main --head <fork-owner>:prepare-js-rattler-v<version> --draft --title "chore: prepare js-rattler v<version> release" --body-file <file>
```

Amending later is `jj squash` then a plain `jj git push` (the bookmark follows the rewrite), or `git commit --amend` and `git push --force-with-lease`.

**Keep the body short.** Link the rendered changelog near the top, then write only what it doesn't say. Half a screen, and if a sentence is already true in the changelog, cut it rather than reword it.

```markdown
**[changelog](https://github.com/<fork-owner>/rattler/blob/<branch>/js-rattler/CHANGELOG.md)**
```

Include the version and the one or two changes that forced the bump level, the judgment calls from step 4 with an invitation to push back (that part exists nowhere else, and it's the only bit a reviewer can really check you on), the pending checklist under `### Pending before release`, whether the snippets ran, a line saying that after merge the `JS Release` workflow is dispatched with `tag: <version>`, and the template's AI disclosure with `Tools:`. Don't paste the user's prompt. `.claude/CLAUDE.md` suggests it, but on a release PR it's long and buries everything that matters. Leave the disclosure boxes for the maintainer to tick.

Reference PRs bare (`#2700`), never with a copied title. GitHub attaches the live title, a copied one goes stale the moment someone retitles. Where the reader needs to know what a PR does to them, describe the effect instead ("`simpleSolve()` rejects unknown platforms"); that's content, not a title.

Reread the body before posting and rewrite whatever sounds like AI wrote it: spaced em dashes as an all-purpose connector, bolded lead-ins, every paragraph the same length, sentences built in neat parallel. Break it up until it reads like a person wrote it. Short sentences are fine, and saying a caveat bothers you beats dressing it up as a limitation.

Draft as long as anything is pending, and `gh pr ready <n> --repo conda/rattler` only once the maintainer confirms. If nothing is pending, still open as draft and ask.

When a pending PR lands the notes are stale. Rebase onto `main@upstream`, re-run steps 2 to 4 over the new commits, add entries for whatever API arrived, and say in the PR which items are already folded in.

## Checklist

- [ ] fresh worktree or workspace on `upstream/main` with tags fetched, not the checkout you had open
- [ ] whole range triaged, including the bundled crates, drops deliberate
- [ ] each breaking candidate argued with its call and traced code path, asked one at a time, confirmed
- [ ] bump level justified
- [ ] `package.json` and `package-lock.json` agree, `Cargo.toml` and `pixi.lock` untouched
- [ ] changelog dated, grouped, linked, breaking marked and called out in the highlights, `pixi run fmt` clean
- [ ] snippets run against the build, or reported as unverified
- [ ] open PRs touching `js-rattler/` checked and pending work listed
- [ ] PR body links the changelog, repeats none of it, says how to publish, humanized
- [ ] opened as a draft from the fork against `conda/rattler:main`
