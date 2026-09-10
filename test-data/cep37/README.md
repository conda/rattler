# CEP-37 compatibility fixtures

The upstream fixtures are copied from [`conda/conda-lock` at
`350c4a04b520104deda43eddc24da29784681f3c`](https://github.com/conda/conda-lock/tree/350c4a04b520104deda43eddc24da29784681f3c).
Their license is included in `LICENSE`.

| Local file | Upstream path |
| --- | --- |
| `multiple-categories.yml` | `tests/test-multiple-categories/conda-lock.yml` |
| `pip-deps.yml` | `tests/test-install-with-pip-deps/conda-lock.yml` |
| `legacy-lockfile.yml` | `tests/test-lockfile/conda-lock.yml` |
| `upgrade-v2.5.8.yml` | `tests/test-v2-to-v3-upgrade/conda-lock-v2.5.8.yml` |
| `upgrade-v3.0.2.yml` | `tests/test-v2-to-v3-upgrade/conda-lock-v3.0.2.yml` |
| `upgrade-v3.0.3.yml` | `tests/test-v2-to-v3-upgrade/conda-lock-v3.0.3.yml` |
| `blas-mkl.yml` | `tests/test-environment-blas-mkl/conda-lock.yml` |
| `explicit-toposorted.yml` | `tests/test-explicit-toposorted/conda-lock.yml` |

`cep-example.yml` is the example from [CEP-37](https://github.com/conda/ceps/blob/main/cep-0037.md),
retrieved on 2026-09-09. CEP text and examples are CC0.

The tests compare parsed YAML semantics, normalizing documented defaults,
channel shorthand, absent/null optional fields, and unconstrained `*` Python
requirements. Comments, mapping order, and package order are not significant.
Metadata hashes are compared as stored, not recomputed: upstream content hashes
represent input specifications, not the resolved package list.

The upgrade fixtures contain Git-sourced Python packages. They must survive
CEP read/write, but offline conversion to a pixi distribution may fail rather
than treating a Git repository as a downloadable wheel. SHA256-only conda
packages are valid CEP inputs even though the pinned conda-lock implementation
requires MD5; they are covered separately from upstream acceptance checks.
