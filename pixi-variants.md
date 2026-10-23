# Variant-dependent build configuration in Pixi

## Problem statement

`py-rattler` normally builds a Python Stable ABI (`abi3`) extension. Its Pixi
package configuration therefore contains:

```toml
[package.build.config]
abi3 = true
compilers = ["c", "rust"]
```

We also want to build and test `py-rattler` with free-threaded CPython, for
example Python 3.14t. A first attempt added an environment constraint like:

```toml
[feature.py314t.dependencies]
python_abi = { version = "*", build = "*_cp314t" }
```

This makes `pixi lock` fail. Enabling `abi3` causes `pixi-build-python` to mark
the package as version-independent, add `python-abi3`, and ignore the normal
CPython ABI run export. In conda metadata this leads to the dependency chain:

```text
py-rattler
└── _python_abi3_support
    └── python-gil
        └── python_abi *_cp314
```

The new environment simultaneously requests:

```text
python_abi *_cp314t
```

The GIL and free-threaded ABI requirements are mutually exclusive, so the
solver correctly reports that the environment is unsatisfiable.

There is a second, independent issue in the attempted environment. It combines
the `test` feature, which pins `python = "3.10.*"`, with the `py314t` feature.
Installing only the `python_abi` metapackage does not ensure that the selected
interpreter is Python 3.14t. A real free-threaded test environment should select
the interpreter, for example with:

```toml
python-freethreading = "3.14.*"
```

It must also avoid inheriting the Python 3.10 pin.

## How conda-forge handles this

The conda-forge `py-rattler` recipe builds separate package variants:

- ordinary CPython builds use `is_abi3 = true`;
- `python = 3.14.* *_cp314t` builds use `is_abi3 = false`.

The Python and `is_abi3` values are correlated with `zip_keys`. Conditional
requirements and `build.python.version_independent` then produce either an
abi3 package or a normal CPython-ABI package. Extra abi3 builds are skipped so
only the minimum supported Python produces the version-independent artifact.

See:

- <https://github.com/conda-forge/py-rattler-feedstock/blob/main/recipe/recipe.yaml>
- <https://github.com/conda-forge/py-rattler-feedstock/blob/main/recipe/conda_build_config.yaml>

Pixi already has most of the machinery needed to represent the resulting
outputs. A build backend can return multiple outputs with the same package name
as long as their variant maps differ. Pixi exposes all matching outputs to the
solver, which can select the one compatible with the requested Python ABI.

The missing capability is making backend configuration depend on a build
variant. `abi3` is currently one static boolean for the entire source package.

## Current Pixi architecture

The relevant flow is:

```text
package.build.config
        │
        ▼
backend initialize (once, with static configuration)
        │
        ▼
conda/outputs (receives the complete variant matrix)
        │
        ▼
GenerateRecipe (runs once with the static configuration)
        │
        ▼
rattler-build renders all variants
        │
        ▼
outputs are added to source repodata and selected by the solver
```

In the Pixi source tree:

- `PackageBuild` stores one opaque backend configuration and optional
  target-specific configurations in
  [`crates/pixi_manifest/src/build_system.rs`](../pixi/crates/pixi_manifest/src/build_system.rs).
- The static configuration is sent to the backend in `InitializeParams` in
  [`crates/pixi_build_types/src/procedures/initialize.rs`](../pixi/crates/pixi_build_types/src/procedures/initialize.rs).
- The variant matrix is sent later in `CondaOutputsParams` in
  [`crates/pixi_build_types/src/procedures/conda_outputs.rs`](../pixi/crates/pixi_build_types/src/procedures/conda_outputs.rs).
- `IntermediateBackend::conda_outputs` merges only target-specific
  configuration, generates one recipe, and then renders the complete matrix in
  [`crates/pixi_build_backend/src/intermediate_backend.rs`](../pixi/crates/pixi_build_backend/src/intermediate_backend.rs).
- `conda/build_v1` repeats the same static target-configuration selection when
  building the selected output.
- `pixi-build-python` adds `python-abi3`, suppresses Python run exports, and
  sets `build.python.version_independent` whenever `abi3 == true` in
  [`crates/pixi_build_python/src/main.rs`](../pixi/crates/pixi_build_python/src/main.rs).

Multiple same-name outputs are already supported. Pixi validates that their
variant maps are unique in
[`crates/pixi_command_dispatcher/src/build_backend_metadata/mod.rs`](../pixi/crates/pixi_command_dispatcher/src/build_backend_metadata/mod.rs),
and returns all same-name outputs from
[`crates/pixi_command_dispatcher/src/keys/source_metadata.rs`](../pixi/crates/pixi_command_dispatcher/src/keys/source_metadata.rs).

This means the solver and lockfile model are fundamentally suitable. The
limitation occurs before solving, while backend metadata is generated.

## Proposed feature: variant-conditioned backend configuration

The general feature should not be specific to Python or abi3. Any backend may
need different settings for compiler, CUDA, Python ABI, linkage, or another
build variant.

### Manifest syntax

Add ordered configuration overrides selected by build variants:

```toml
[workspace.build-variants]
python = [
  "3.10.* *_cp310",
  "3.14.* *_cp314t",
]

[package.build.config]
abi3 = true
compilers = ["c", "rust"]

[[package.build.config-overrides]]
when = { python = "3.14.* *_cp314t" }
config = { abi3 = false }
```

The initial implementation can use exact matching against declared variant
values. It can later accept multiple allowed values or richer structured
matchers:

```toml
[[package.build.config-overrides]]
when = { python = [
  "3.14.* *_cp314t",
  "3.15.* *_cp315t",
] }
config = { abi3 = false }
```

Arbitrary Jinja expressions should not be part of the manifest API. Structured
selectors are easier to validate, hash, explain, and evolve.

### Resolution semantics

For each concrete build-variant assignment, effective configuration is resolved
in this order:

1. base `[package.build.config]`;
2. matching target configuration;
3. matching variant overrides, in declaration order.

Multiple entries in `when` are ANDed. Referring to a variant key that does not
exist in the effective variant configuration is an error.

Configuration patches should use a backend-independent and documented merge
model. JSON Merge Patch semantics are a reasonable choice:

- objects merge recursively;
- arrays and scalar values replace existing values;
- `null` removes a value.

The fully merged value is deserialized into the backend's typed configuration.
This is preferable to deserializing each patch independently because a partial
patch may omit fields required by a backend.

## Backend execution model

`IntermediateBackend::conda_outputs` should no longer assume that a single
configuration generates the entire matrix. Instead it should:

1. Load backend default variants.
2. Load variant files, retaining correlations such as `zip_keys`.
3. Apply workspace-supplied variants.
4. Expand the result into concrete variant assignments.
5. Resolve effective backend configuration for each assignment.
6. Group assignments that have identical effective configurations.
7. Generate a recipe once for each configuration group.
8. Render that recipe only for the assignments in its group.
9. Combine and validate all rendered outputs.

Grouping is important. For example, Python 3.10 through 3.13 might all resolve
to the same abi3 configuration, while Python 3.14t resolves to a non-abi3
configuration. Pixi should generate two recipes rather than one recipe per
Python version.

Conceptually:

```rust
for group in group_by_effective_config(variant_assignments) {
    let config = deserialize(group.resolved_config)?;
    let recipe = generator.generate_recipe(..., &config, ...).await?;
    outputs.extend(render(recipe, group.assignments)?);
}
```

This may require a rattler-build rendering entry point that accepts explicit
variant assignments. Reconstructing each group as independent per-key lists can
lose correlations and accidentally recreate a Cartesian product.

### Build-time consistency

`conda/build_v1` must use the same configuration resolution algorithm as
`conda/outputs`. Otherwise Pixi could solve against metadata produced with one
configuration and build the selected artifact with another.

The selected output should be matched by its complete identity:

```text
name + version + build string + subdir + variant map
```

The current build path primarily finds an output through its variant map.
Matching the complete identity is safer once backend configuration can create
multiple output families.

If two effective configurations produce the same package name and variant map
but different metadata, Pixi should report an ambiguity rather than choosing an
output based on ordering.

## Protocol changes

Add an optional ordered list to the initialization model:

```rust
pub struct ConfigurationOverride {
    pub when: BTreeMap<String, VariantMatcher>,
    pub patch: serde_json::Value,
}

pub struct InitializeParams {
    // Existing fields...
    pub configuration_overrides: Option<Vec<ConfigurationOverride>>,
}
```

Support should be advertised through build API capability negotiation. If a
manifest uses configuration overrides with an older backend, Pixi should fail
with a direct diagnostic rather than silently ignoring them:

```text
backend pixi-build-foo does not support variant-conditioned configuration
```

The new rules must participate in:

- `PackageBuild::hash`;
- the backend `ConfigurationHash`;
- metadata cache invalidation;
- generated JSON schema and manifest documentation.

Out-of-tree backends may implement the protocol themselves. The shared
`pixi_build_backend::IntermediateBackend` should provide the full resolution,
grouping, and rendering implementation for the in-tree backends and for
third-party backends that use the helper crate.

## Correlated inline variants

Variant-conditioned configuration can initially match the complete Python
variant value directly. Longer term, Pixi should support correlated inline
variant sets rather than requiring variant files with conda-build `zip_keys`.

A row-oriented representation is clearer than parallel arrays:

```toml
[[workspace.build-variant-sets]]
python = "3.10.* *_cp310"
python-abi-kind = "gil"

[[workspace.build-variant-sets]]
python = "3.14.* *_cp314t"
python-abi-kind = "freethreaded"
```

The package can then select a semantic variant rather than matching a Python
build string:

```toml
[[package.build.config-overrides]]
when = { python-abi-kind = "freethreaded" }
config = { abi3 = false }
```

This generalizes naturally to correlated compiler, CUDA, MPI, linkage, and ABI
configurations. It can be implemented separately from configuration overrides;
exact matching on the Python variant is sufficient for the first version.

## Python-backend considerations

### Implicit Python must consume the Python variant

During experimentation, a non-abi3 package did not build against the requested
Python 3.14t variant until `python = "*"` was added explicitly to
`[package.host-dependencies]`. The Python backend's automatically inserted host
Python should participate in the workspace `python` build variant in the same
way as an explicit host dependency.

Otherwise an environment can request Python 3.14t while package metadata is
generated against the backend's default or minimum ordinary Python.

This should be fixed independently of conditional configuration.

### Optional `abi3 = "auto"` sugar

After the generic feature exists, `pixi-build-python` could offer:

```toml
[package.build.config]
abi3 = "auto"
```

This could mean “use abi3 for ordinary CPython and the full CPython ABI for
free-threaded builds.” It would be convenient for common Python projects, but it
should be implemented on top of the generic variant-aware machinery rather
than becoming a one-off execution path.

There may also be policy details that make explicit overrides preferable. For
example, a future free-threaded stable ABI may be available only from a certain
Python version, and projects may deliberately choose different minimum ABIs.

## Short-term workarounds

Until Pixi supports variant-conditioned configuration, there are three viable
approaches.

### Separate internal source packages

Keep the root package as abi3 and add a second package manifest with a distinct
internal conda package name, the same source directory, and `abi3 = false`.
Make the Python 3.14t environment depend on the second package. The wheel still
installs the `rattler` Python module; only the internal conda distribution name
needs to differ.

This approach was validated in a temporary workspace. `pixi lock` successfully
resolved both an abi3/Python 3.10 environment and a non-abi3/Python 3.14t
environment on Linux, macOS Intel, macOS ARM, and Windows.

### Use `pixi-build-rattler-build`

A local recipe can express the same conditional output and zipped variants as
the conda-forge recipe. This is flexible but duplicates recipe logic that the
Python backend is intended to generate.

### Build the free-threaded extension in a task

The Python 3.14t environment can omit the local conda source package and run
`maturin develop` before the test suite. This avoids a second package manifest,
but bypasses testing Pixi's package-build metadata and is less representative
of installation from a built conda package.

## Alternatives considered

### Make `abi3` automatically disable itself for `cp*t`

This solves the immediate Python case but not the underlying architectural
limitation. Other backends will need variant-dependent settings as well. It can
be useful sugar after the generic capability exists.

### Let features override package build configuration

Features describe environments, while one source package can be used by many
environments and must expose all compatible outputs to the solver. Making the
package definition depend on whichever feature first requested it introduces
order dependence and complicates shared solve groups. Build variants are the
appropriate dimension.

### Have the frontend instantiate one backend per variant

The frontend could expand the matrix, resolve configuration, and initialize a
new backend process for every assignment. This avoids changing the backend
protocol but has significant drawbacks:

- variant defaults and variant files are currently interpreted by the backend;
- it can start many duplicate backend processes;
- it moves rattler-build variant semantics into the frontend;
- grouping equivalent configurations becomes harder;
- third-party backends may have their own variant-generation logic.

Keeping matrix expansion and recipe generation in the backend, with a shared
implementation in `IntermediateBackend`, preserves the existing ownership
model.

## Suggested implementation sequence

1. Add manifest parsing, schema generation, diagnostics, and hashing for
   `package.build.config-overrides`.
2. Define structured variant matchers and a deterministic configuration patch
   resolver.
3. Add build protocol types and capability negotiation.
4. Refactor `IntermediateBackend::conda_outputs` to expand assignments and
   group them by effective configuration.
5. Add or expose a renderer API that preserves explicit correlated variant
   assignments.
6. Apply the identical resolver in `conda/build_v1`.
7. Strengthen selected-output matching to use full output identity.
8. Fix the Python backend's implicit host Python so it consumes the `python`
   build variant.
9. Add an end-to-end fixture containing one abi3 output and one `cp314t`
   non-abi3 output, and assert that the solver selects each in the appropriate
   environment.
10. Add cache tests showing that changes to selectors or patches invalidate
    backend metadata and built artifacts.
11. Consider correlated inline variant sets and Python-specific `abi3 = "auto"`
    as follow-up ergonomics.

## Desired end state for `py-rattler`

With the generic feature, the relevant configuration could look like:

```toml
[workspace.build-variants]
python = [
  "3.10.* *_cp310",
  "3.14.* *_cp314t",
]

[package.host-dependencies]
python = "*"
maturin = ">=1.14.0,<2"

[package.build.config]
abi3 = true
compilers = ["c", "rust"]

[[package.build.config-overrides]]
when = { python = "3.14.* *_cp314t" }
config = { abi3 = false }

[feature.py310.dependencies]
py-rattler = { path = "." }
python = "3.10.*"

[feature.py314t.dependencies]
py-rattler = { path = "." }
python-freethreading = "3.14.*"

[environments]
test = { features = ["test", "py310"] }
py314t = { features = ["test", "py314t"] }
```

The backend would expose an abi3 output compatible with ordinary CPython and a
normal CPython-ABI output built against `cp314t`. The existing solver would then
select the correct output from the same source package for each environment.
