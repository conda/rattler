# Sigstore attestation fixture

A real conda package and the attestation sidecar it was published with, used by
the `rattler verify-attestation` snapshot tests in
`crates/rattler-bin/tests/cli.rs`. Both files are kept byte for byte as they are
served, since changing either invalidates the signature.

They were downloaded from the `skill-forge` channel on prefix.dev:

```bash
base=https://prefix.dev/skill-forge/noarch/agent-skill-conda-forge-0.0.21-h4616a5c_0.conda
curl -sSL -o agent-skill-conda-forge-0.0.21-h4616a5c_0.conda "$base"
curl -sSL -o agent-skill-conda-forge-0.0.21-h4616a5c_0.conda.sigs "$base.sigs"
```

The package was built and signed by the `pavelzw/skill-forge` GitHub Actions
workflow, so the signing certificate carries the full set of Fulcio CI claims
that the snapshots show. Verification is pinned to the time the signature was
recorded in the transparency log, so the fixture keeps verifying after the
short-lived certificate expires.
