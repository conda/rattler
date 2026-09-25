# Sigstore attestations

Py-rattler can verify the Sigstore attestations advertised by conda package records and apply the same policy while installing packages.

Attestations use the format and verification rules of [CEP 27](https://conda.org/learn/ceps/cep-0027) and are distributed as described by [CEP 50](https://conda.org/learn/ceps/cep-0050): a `.sigs` sidecar next to the package, advertised through the `attestations_sha256` field of the repodata. Which signing identities to trust is not covered by either CEP and is up to the policy you pass in.

```python
from rattler import Issuer, Publisher, VerificationPolicy

publisher = Publisher(
    identity="https://github.com/conda/rattler/*",
    issuer=Issuer.github_actions(),
)
policy = VerificationPolicy.require(publisher)
```

Pass the policy to `verify_attestation` to inspect one record, or to `install` to verify every package installed, replaced, or relinked by a transaction. Installer verification is completed before package metadata or files are changed. Unchanged and removal-only packages are outside the policy's scope.

```python
outcome = await verify_attestation(record, policy)
await install(records, target_prefix, attestation_policy=policy)
```

## Pinning the trust anchors

By default the production Sigstore trusted root is fetched over TUF the first time a bundle is verified, which needs access to `tuf-repo-cdn.sigstore.dev`. Passing a `TrustedRoot` verifies against that trust material instead, so the only remaining request is the sidecar download.

```python
from rattler import TrustedRoot

trusted_root = TrustedRoot.from_path("trusted_root.json")
outcome = await verify_attestation(record, policy, trusted_root=trusted_root)
```

A pinned root stays usable as the Sigstore infrastructure rotates its keys, because a bundle's certificate chain is validated against the point in time it was signed at. It does not pick up new certificate authorities, so a bundle issued under one the file does not carry fails to verify until the file is refreshed.

If you have no `trusted_root.json` to point at, `TrustedRoot.embedded()` returns the public good instance's trust anchors that ship with py-rattler, which is what makes verification work with no network access beyond the sidecar at all.

```python
outcome = await verify_attestation(record, policy, trusted_root=TrustedRoot.embedded())
```

Reach for this when `tuf-repo-cdn.sigstore.dev` is unreachable rather than as a general way to skip the network. The snapshot is taken when py-rattler's Sigstore dependencies are released, so it carries the same staleness caveat as any pinned root and can only be refreshed by upgrading py-rattler.

::: rattler.sigstore
