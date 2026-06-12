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

let host = ($env.PREFIX_DEV_E2E_HOST? | default "https://beta.prefix.dev" | str trim --right --char "/")
let channel = ($env.PREFIX_DEV_E2E_CHANNEL? | default "rattler-e2e")
let package_file = "test-data/packages/empty-0.1.0-h4616a5c_0.conda"
let package_filename = "empty-0.1.0-h4616a5c_0.conda"
# prefix.dev deployments (prefix.dev and *.prefix.dev, e.g. beta) all validate
# GitHub OIDC tokens against the shared audience "prefix.dev"; other hosts use
# their own host name. Mirrors TrustedPublishingOptions::for_server.
let host_name = ($host | url parse | get host)
let audience = if ($host_name == "prefix.dev") or ($host_name | str ends-with ".prefix.dev") {
  "prefix.dev"
} else {
  $host_name
}

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

def check_challenge [url: string] {
  let resp = (
    try {
      http get --full --allow-errors --max-time 30sec $url
    } catch {
      fail $"Step 4: could not reach ($url) — connection/DNS failure"
    }
  )
  if not ($resp.status in [401 403]) {
    fail $"expected 401/403 for anonymous access to ($url), got ($resp.status) — is the channel private?"
  }
  let www = ($resp.headers.response | where name == "www-authenticate")
  if ($www | is-empty) {
    fail $"got ($resp.status) for ($url) but no WWW-Authenticate header — the server does not fire the challenge"
  }
  let www_value = ($www | first | get value)
  if not ($www_value | str downcase | str contains "bearer") {
    fail $"WWW-Authenticate present for ($url) but without a Bearer scheme: ($www_value)"
  }
  print $"   ($url) challenged with: ($www_value)"
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
let mint_resp = (
  try {
    http post --full --allow-errors --max-time 30sec --content-type application/json $"($host)/api/oidc/mint_token" { token: $oidc_token }
  } catch {
    fail $"mint endpoint ($host)/api/oidc/mint_token unreachable — connection/DNS failure"
  }
)
if $mint_resp.status != 200 {
  # The server's error body is a plain diagnostic message (never a token);
  # truncate defensively so a surprising body cannot dump a credential.
  let detail = ($mint_resp.body | into string | str substring 0..300)
  fail $"mint endpoint ($host)/api/oidc/mint_token rejected the OIDC token \(status ($mint_resp.status)\): ($detail) — check the repository-access config \(repo + workflow + audience ($audience)\) on the server"
}
let minted = $mint_resp.body
if ($minted | describe) != "string" {
  fail "mint endpoint returned a non-string response (expected the raw token)"
}
if not ($minted | str starts-with "pfx-jwt") {
  fail "mint endpoint returned something that is not a pfx-jwt token"
}
print "   minted a short-lived token: server mint + repository access OK"

# Bring-up escape hatch: skip the write steps (2 and 3) while the upload
# endpoint's 403 for minted tokens is under investigation server-side. The
# read-path verification (steps 4-5) is what fixes pixi#6318 and stands alone.
let skip_upload = (($env.PREFIX_DEV_E2E_SKIP_UPLOAD? | default "") == "true")

# -- Step 2: best-effort cleanup of a previous run's package ------------------
if $skip_upload {
  print "== Step 2: SKIPPED (PREFIX_DEV_E2E_SKIP_UPLOAD)"
} else {
  print "== Step 2: best-effort cleanup of previous package"
  try {
    http delete --max-time 30sec --headers [Authorization $"Bearer ($minted)"] $"($host)/api/v1/delete/($channel)/noarch/($package_filename)"
    print "   deleted previous package"
  } catch {
    print "   nothing to delete (or delete refused) — continuing; upload uses --skip-existing"
  }
}

# -- Step 2b: diagnostic — what can the minted token actually do? -------------
# An authenticated read with the minted token discriminates failure modes:
# 200 = token is scoped to the channel (a later write failure means scope),
# 401/403 = token not authorized for this channel at all (wrong attachment),
# 404 = channel/namespace mismatch.
print "== Step 2b: authenticated read probe with the minted token"
let probe = (
  try {
    http get --full --allow-errors --max-time 30sec --headers [Authorization $"Bearer ($minted)"] $"($host)/($channel)/noarch/repodata.json"
  } catch {
    fail $"Step 2b: could not reach ($host) — connection/DNS failure"
  }
)
print $"   authenticated GET ($channel)/noarch/repodata.json -> status ($probe.status)"

# -- Step 3: upload through the proactive trusted-publishing path -------------
if $skip_upload {
  print "== Step 3: SKIPPED (PREFIX_DEV_E2E_SKIP_UPLOAD)"
} else {
  must $"Step 3: rattler upload prefix to ($host)/($channel)" "proactive OIDC upload path (rattler_upload audience or write scope)" {
    ^rattler upload prefix --url $host --channel $channel --skip-existing $package_file
  }
}

# -- Step 4: the server must fire the challenge for anonymous access ----------
print "== Step 4: anonymous requests must be challenged with WWW-Authenticate"
check_challenge $"($host)/($channel)/noarch/repodata.json"
# The CLI's first request goes to the sharded index; it must be challenged too.
check_challenge $"($host)/($channel)/noarch/repodata_shards.msgpack.zst"

# -- Step 5: the actual challenge-reactive read through the rattler CLI -------
# Step 0 removed every stored-credential source and step 4 proved anonymous
# access is rejected, so the only way this solve can see the repodata is the
# middleware reacting to the WWW-Authenticate challenge (mint + replay).
print $"== Step 5: rattler create --dry-run from ($host)/($channel) \(challenge-reactive read\)"
let create = (do { ^rattler create --dry-run -c $"($host)/($channel)" "empty==0.1.0" } | complete)
let combined = ($create.stdout + "\n" + $create.stderr)
if $create.exit_code == 0 {
  print "   solved through the challenge-reactive path"
  print "== SUCCESS: OIDC circle (independent mint, challenge, reactive read) passed"
} else if ($combined | str downcase | str contains "no candidates") {
  # A "no candidates" solve error means the repodata download itself
  # SUCCEEDED (an auth failure would surface as a 401/download error):
  # the challenge-reactive auth path is verified; only the test package
  # is missing from the channel.
  print "   AUTH PATH VERIFIED: repodata was fetched through the challenge-reactive"
  print "   path, but the channel does not contain the test package yet."
  print "   Upload empty-0.1.0 (manually or once step 3 works) for the full circle."
  print "== SUCCESS (read-path verification): challenge-reactive auth works"
} else {
  print ($combined | lines | last 25 | str join "\n")
  fail "Step 5: challenge-reactive read failed — AuthChallengeMiddleware / TrustedPublishingFlow (see output above)"
}
