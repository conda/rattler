# Packages

Commands that work with conda packages and channel data — searching, inspecting, downloading, and extracting.

---

## `rattler search` — search channels

Searches the configured channels for records matching a pattern. The pattern is parsed as a match spec and supports exact names, glob (`python*`, `*ssl*`), and regex (`^numpy-.*$`).

```
rattler search [OPTIONS] <MATCHSPEC>
```

| Option | Description |
|--------|-------------|
| `<MATCHSPEC>` | Exact name, glob, or regex pattern. |
| `-c`, `--channels <CHANNEL>` | Channels to search (repeatable). Default: `conda-forge`. |
| `-p`, `--platform <PLATFORM>` | Platform. Default: current. `noarch` is always included. |
| `--limit-packages <N>` | Max distinct packages to show. Default: `3`. |
| `--limit <N>` | Max versions to show per package. Default: `5`. |
| `--all` | Remove both limits. |
| `--sharded <true\|false>` | Use sharded repodata. Default: `true`. |

**Examples:**

```bash
rattler search 'python*'
rattler search '^numpy-.*$'
rattler search openssl -c bioconda
rattler search 'polars' --platform linux-64 --all
```

---

## `rattler inspect` — inspect a remote `.conda` package

Streams `info/index.json` and `info/paths.json` from a remote `.conda` archive and prints the metadata plus the first 10 installed paths.

```
rattler inspect <URL>
```

Must be a `.conda` archive URL (not `.tar.bz2`). Auth is read from the rattler auth storage, so private channels work after `rattler auth login`.

**Example:**

```bash
rattler inspect https://conda.anaconda.org/conda-forge/noarch/tqdm-4.66.5-pyhd8ed1ab_0.conda
```

---

## `rattler fetch-file` — read a file from inside a remote package

Streams a single file out of a remote `.conda` or `.tar.bz2` archive and writes its bytes to stdout. Useful for grabbing `info/recipe/recipe.yaml`, `info/about.json`, or a specific library without downloading the whole package.

```
rattler fetch-file <URL> <PATH>
```

| Argument | Description |
|----------|-------------|
| `<URL>` | URL of the conda package archive. |
| `<PATH>` | Path inside the package (e.g. `info/index.json`, `lib/libfoo.so`). |

Exits non-zero if the file is not found.

**Example:**

```bash
rattler fetch-file \
  https://conda.anaconda.org/conda-forge/noarch/tqdm-4.66.5-pyhd8ed1ab_0.conda \
  info/about.json | jq
```

---

## `rattler download` — download any URL (auth-aware)

Downloads a file using the rattler HTTP client stack (so auth / mirror middleware apply). Pipe to stdout with `-o -`.

```
rattler download [-o <OUTPUT>] <URL>
```

| Option | Description |
|--------|-------------|
| `<URL>` | URL to download. |
| `-o`, `--output <PATH>` | Output path, or `-` for stdout. Default: filename inferred from the URL. |

**Example:**

```bash
rattler download https://example.com/my-package.conda
rattler download -o - https://example.com/repodata.json | jq
```

---

## `rattler extract` — extract a local or remote archive

Extracts a `.conda` or `.tar.bz2` package. The argument may be a local path or a URL.

```
rattler extract [-d <DESTINATION>] <PACKAGE>
```

| Option | Description |
|--------|-------------|
| `<PACKAGE>` | Path or URL to the archive. |
| `-d`, `--destination <PATH>` | Output directory. Defaults to a directory named after the package (without the `.conda` / `.tar.bz2` suffix). |

After extraction the SHA-256, MD5, and total size are printed.

**Example:**

```bash
rattler extract ./tqdm-4.66.5-pyhd8ed1ab_0.conda
rattler extract -d /tmp/pkg https://conda.anaconda.org/conda-forge/noarch/tqdm-4.66.5-pyhd8ed1ab_0.conda
```

---

## `rattler link` — link an extracted package into a prefix

Given a directory produced by `rattler extract` (containing `info/` and the package files), link its contents into a prefix. Useful for hand-assembling a prefix or debugging the install step.

```
rattler link -d <DESTINATION> <PACKAGE_DIR>
```

| Option | Description |
|--------|-------------|
| `<PACKAGE_DIR>` | Directory of the extracted package. |
| `-d`, `--destination <PATH>` | Prefix to link into. Created if it does not exist. |

**Example:**

```bash
rattler extract ./tqdm-4.66.5-pyhd8ed1ab_0.conda -d /tmp/tqdm
rattler link -d ./env /tmp/tqdm
```
