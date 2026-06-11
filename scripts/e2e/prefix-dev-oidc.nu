#!/usr/bin/env nu
# End-to-end test of the prefix.dev OIDC flow (see
# docs/superpowers/specs/2026-06-11-prefix-dev-oidc-e2e-design.md).
#
# Steps are ordered so the first failure names the broken component:
#   1. independent mint            -> repository-access config / mint endpoint
#   2. best-effort cleanup         -> (tolerant)
#   3. upload (proactive OIDC)     -> rattler_upload audience / write scope
#   4. anonymous challenge check   -> server WWW-Authenticate behavior
#   5. challenge-reactive read     -> AuthChallengeMiddleware / TrustedPublishingFlow

let host = ($env.PREFIX_DEV_E2E_HOST? | default "https://beta.prefix.dev")
let channel = ($env.PREFIX_DEV_E2E_CHANNEL? | default "rattler-e2e")
let package_file = "test-data/packages/empty-0.1.0-h4616a5c_0.conda"
let package_filename = "empty-0.1.0-h4616a5c_0.conda"
let audience = ($host | url parse | get host)

def fail [msg: string] {
  print $"FAIL: ($msg)"
  exit 1
}

# Run an external command; hard-fail with `indicts` if it exits non-zero.
def must [desc: string, indicts: string, cmd: closure] {
  print $"== ($desc)"
  try { do $cmd } catch { }
  let code = ($env.LAST_EXIT_CODE? | default 0)
  if $code != 0 {
    fail $"($desc) failed \(exit=($code)\) — ($indicts)"
  }
}

# -- Step 0: preconditions ---------------------------------------------------
# Hosted runners have an empty keyring; guard the file-based sources that
# could otherwise satisfy auth and silently bypass the challenge path.
# Default FileStorage path: ~/.rattler/credentials.json
# (see crates/rattler_networking/src/authentication_storage/backends/file.rs)
if ($env.RATTLER_AUTH_FILE? | default "" | is-not-empty) {
  fail "RATTLER_AUTH_FILE is set; this test must run without stored credentials"
}
if ($env.NETRC? | default "" | is-not-empty) {
  fail "NETRC is set; this test must run without stored credentials"
}
let default_credentials = ($nu.home-path | path join ".rattler" "credentials.json")
if ($default_credentials | path exists) {
  fail $"($default_credentials) exists; this test must run without stored credentials"
}
if ($env.ACTIONS_ID_TOKEN_REQUEST_URL? | default "" | is-empty) {
  fail "ACTIONS_ID_TOKEN_REQUEST_URL missing; the job needs `permissions: id-token: write`"
}
if ($env.ACTIONS_ID_TOKEN_REQUEST_TOKEN? | default "" | is-empty) {
  fail "ACTIONS_ID_TOKEN_REQUEST_TOKEN missing; the job needs `permissions: id-token: write`"
}

# -- Step 1: independent mint (proves server half without any rattler code) --
print $"== Step 1: independent OIDC mint against ($host) \(audience ($audience)\)"
let oidc_token = (
  try {
    http get --max-time 30sec --headers [Authorization $"bearer ($env.ACTIONS_ID_TOKEN_REQUEST_TOKEN)"] $"($env.ACTIONS_ID_TOKEN_REQUEST_URL)&audience=($audience)"
      | get value
  } catch {
    fail "could not fetch the GitHub Actions OIDC token — runner/permissions problem"
  }
)
let minted = (
  try {
    http post --max-time 30sec --content-type application/json $"($host)/api/oidc/mint_token" { token: $oidc_token }
  } catch {
    fail $"mint endpoint ($host)/api/oidc/mint_token rejected the OIDC token — check the repository-access config \(repo + audience ($audience)\) on the server"
  }
)
if ($minted | describe) != "string" {
  fail "mint endpoint returned a non-string response (expected the raw token)"
}
if not ($minted | str starts-with "pfx-jwt") {
  fail "mint endpoint returned something that is not a pfx-jwt token"
}
print "   minted a short-lived token: server mint + repository access OK"

# -- Step 2: best-effort cleanup of a previous run's package ------------------
print "== Step 2: best-effort cleanup of previous package"
try {
  http delete --max-time 30sec --headers [Authorization $"Bearer ($minted)"] $"($host)/api/v1/delete/($channel)/noarch/($package_filename)"
  print "   deleted previous package"
} catch {
  print "   nothing to delete (or delete refused) — continuing; upload uses --skip-existing"
}

# -- Step 3: upload through the proactive trusted-publishing path -------------
must $"Step 3: rattler upload prefix to ($host)/($channel)" "proactive OIDC upload path (rattler_upload audience or write scope)" {
  ^rattler upload prefix --url $host --channel $channel --skip-existing $package_file
}

# -- Step 4: the server must fire the challenge for anonymous access ----------
print "== Step 4: anonymous request must be challenged with WWW-Authenticate"
let resp = (
  try {
    http get --full --allow-errors --max-time 30sec $"($host)/($channel)/noarch/repodata.json"
  } catch {
    fail $"Step 4: could not reach ($host) — connection/DNS failure"
  }
)
if not ($resp.status in [401 403]) {
  fail $"expected 401/403 for anonymous access, got ($resp.status) — is the channel private?"
}
let www = ($resp.headers.response | where name == "www-authenticate")
if ($www | is-empty) {
  fail $"got ($resp.status) but no WWW-Authenticate header — the server does not fire the challenge"
}
let www_value = ($www | first | get value)
if not ($www_value | str downcase | str contains "bearer") {
  fail $"WWW-Authenticate present but without a Bearer scheme: ($www_value)"
}
print $"   challenged with: ($www_value)"

# -- Step 5: the actual challenge-reactive read through the rattler CLI -------
must $"Step 5: rattler create --dry-run from ($host)/($channel)" "challenge-reactive read path (AuthChallengeMiddleware / TrustedPublishingFlow)" {
  ^rattler create --dry-run -c $"($host)/($channel)" "empty==0.1.0"
}

print "== SUCCESS: full OIDC circle (independent mint, upload, challenge, reactive read) passed"
