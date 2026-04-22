# Authentication

`rattler auth` stores and removes credentials used by every other command (channel access, `upload`, private repodata, etc.). Credentials are persisted to the system keychain when available, otherwise to the rattler auth file.

```
rattler auth <COMMAND>
```

Subcommands: `login`, `logout`.

---

## `rattler auth login`

Stores credentials for a host. The host is a bare domain such as `prefix.dev` or `anaconda.org` — don't include `https://` or a path. Exactly one authentication category should be supplied per invocation.

```
rattler auth login [OPTIONS] <HOST>
```

### Token / basic auth

| Option | Description |
|--------|-------------|
| `--token <TOKEN>` | prefix.dev-style bearer token. |
| `--username <USER>` | HTTP Basic username (pair with `--password`). |
| `--password <PASS>` | HTTP Basic password. |
| `--conda-token <TOKEN>` | anaconda.org / Quetz conda token. |

### S3 credentials

Used for S3-backed channels (`s3://...`) and `rattler upload s3`.

| Option | Description |
|--------|-------------|
| `--s3-access-key-id <ID>` | AWS access key ID. |
| `--s3-secret-access-key <KEY>` | AWS secret access key. |
| `--s3-session-token <TOKEN>` | Optional session token. |

### OAuth / OIDC

| Option | Description |
|--------|-------------|
| `--oauth` | Use OAuth/OIDC. |
| `--oauth-issuer-url <URL>` | OIDC issuer URL. Defaults to `https://<HOST>`. |
| `--oauth-client-id <ID>` | OAuth client ID. Default: `rattler`. |
| `--oauth-client-secret <SECRET>` | For confidential clients. |
| `--oauth-flow <auto\|auth-code\|device-code>` | Flow to use. Default: `auto`. |
| `--oauth-scope <SCOPE>` | Extra scopes (repeatable). |

### Examples

```bash
# prefix.dev token
rattler auth login prefix.dev --token pfx_xxx

# anaconda.org conda token
rattler auth login anaconda.org --conda-token xxx

# HTTP basic
rattler auth login myserver.example.com --username alice --password s3cret

# S3
rattler auth login my-bucket.s3.amazonaws.com \
  --s3-access-key-id AKIA... --s3-secret-access-key ...

# OAuth device-code flow
rattler auth login prefix.dev --oauth --oauth-flow device-code
```

---

## `rattler auth logout`

Removes stored credentials for a host.

```
rattler auth logout <HOST>
```

**Example:**

```bash
rattler auth logout prefix.dev
```
