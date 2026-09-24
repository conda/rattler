# Sigstore attestations

Py-rattler can verify the Sigstore attestations advertised by conda package records and apply the same policy while installing packages.

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

::: rattler.sigstore
