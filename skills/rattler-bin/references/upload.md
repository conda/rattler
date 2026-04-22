# Uploading Packages

`rattler upload` publishes built `.conda` / `.tar.bz2` files to a registry. Each target is its own subcommand with its own auth conventions.

```
rattler upload [OPTIONS] [PACKAGE_FILES]... <COMMAND>
```

Global flags:

| Option | Description |
|--------|-------------|
| `[PACKAGE_FILES]...` | One or more package files to upload. |
| `--allow-insecure-host <HOST>` | Skip TLS verification for the given host (repeatable). |

Credentials for all targets come from the rattler auth storage (the system keychain or the auth file written by `rattler auth login`). The `--api-key` / `--token` flags on each subcommand override that for the current invocation, and all of them also accept the same value via environment variables (shown below).

---

## `rattler upload prefix` — prefix.dev

```
rattler upload prefix -c <CHANNEL> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-c`, `--channel <CHANNEL>` | `PREFIX_CHANNEL` | Target channel on prefix.dev. Required. |
| `-u`, `--url <URL>` | `PREFIX_SERVER_URL` | Server URL. Default: `https://prefix.dev`. |
| `-a`, `--api-key <KEY>` | `PREFIX_API_KEY` | API key. Falls back to stored credentials. |
| `--attestation <FILE>` | | Upload a pre-generated attestation alongside the package (single package only). Mutually exclusive with `--generate-attestation`. |
| `--generate-attestation` | | Generate an attestation with `cosign` in CI. Mutually exclusive with `--attestation`. |
| `--store-github-attestation` | | Also POST the generated attestation to GitHub's attestation API (requires `GITHUB_TOKEN`; GitHub Actions only). |
| `-s`, `--skip-existing` | | Skip packages already present. |
| `--force` | | Overwrite existing packages. |

---

## `rattler upload anaconda` — anaconda.org

```
rattler upload anaconda -o <OWNER> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-o`, `--owner <OWNER>` | `ANACONDA_OWNER` | Username or organization. Required. |
| `-c`, `--channel <CHANNELS>` | `ANACONDA_CHANNEL` | Label/channel (e.g. `main`, `rc`). |
| `-u`, `--url <URL>` | `ANACONDA_SERVER_URL` | Custom Anaconda server. |
| `-a`, `--api-key <KEY>` | `ANACONDA_API_KEY` | API token. Falls back to stored credentials. |
| `-f`, `--force` | `ANACONDA_FORCE` | Replace on conflict. |

---

## `rattler upload quetz` — Quetz

```
rattler upload quetz -u <URL> -c <CHANNELS> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-u`, `--url <URL>` | `QUETZ_SERVER_URL` | Quetz server URL. Required. |
| `-c`, `--channel <CHANNELS>` | `QUETZ_CHANNEL` | Channel on the Quetz server. Required. |
| `-a`, `--api-key <KEY>` | `QUETZ_API_KEY` | API key. Falls back to stored credentials. |

---

## `rattler upload artifactory` — JFrog Artifactory

```
rattler upload artifactory -u <URL> -c <CHANNELS> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-u`, `--url <URL>` | `ARTIFACTORY_SERVER_URL` | Artifactory server URL. Required. |
| `-c`, `--channel <CHANNELS>` | `ARTIFACTORY_CHANNEL` | Channel on the Artifactory server. Required. |
| `-t`, `--token <TOKEN>` | `ARTIFACTORY_TOKEN` | Token. Falls back to stored credentials. |

---

## `rattler upload cloudsmith` — Cloudsmith

```
rattler upload cloudsmith -o <OWNER> -r <REPO> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-o`, `--owner <OWNER>` | `CLOUDSMITH_OWNER` | Cloudsmith namespace. Required. |
| `-r`, `--repo <REPO>` | `CLOUDSMITH_REPO` | Cloudsmith repository. Required. |
| `-a`, `--api-key <KEY>` | `CLOUDSMITH_API_KEY` | API key. Falls back to stored credentials. |
| `-u`, `--url <URL>` | `CLOUDSMITH_API_URL` | API server URL. |

---

## `rattler upload s3` — S3-compatible bucket

```
rattler upload s3 -c <CHANNEL> [OPTIONS] <PACKAGE_FILES>...
```

| Option | Env | Description |
|--------|-----|-------------|
| `-c`, `--channel <CHANNEL>` | `S3_CHANNEL` | `s3://bucket/channel` URL. Required. |
| `--force` | | Overwrite if the object already exists. |

S3 credentials (either stored via `rattler auth login --s3-*`, or passed as flags / env vars):

| Option | Env |
|--------|-----|
| `--endpoint-url <URL>` | `S3_ENDPOINT_URL` |
| `--region <REGION>` | `S3_REGION` |
| `--access-key-id <ID>` | `S3_ACCESS_KEY_ID` |
| `--secret-access-key <KEY>` | `S3_SECRET_ACCESS_KEY` |
| `--session-token <TOKEN>` | `S3_SESSION_TOKEN` |
| `--addressing-style <virtual-host\|path>` | `S3_ADDRESSING_STYLE` — default `virtual-host`. |

---

## Examples

```bash
# prefix.dev
rattler upload prefix -c my-channel ./output/noarch/mypkg-1.0-py_0.conda

# anaconda.org, replacing on conflict
rattler upload anaconda -o myuser --force ./output/linux-64/mypkg-1.0-0.conda

# S3
rattler upload s3 -c s3://my-bucket/my-channel --region eu-central-1 ./dist/*.conda
```
