# User-Specified Virtual Packages

## Proposal for Conda Channel-Defined Virtual Package Plugins

### Status

The **metadata half** of this proposal is implemented in rattler behind the
`experimental-virtual-package-plugins` cargo feature: the registration is parsed from repodata and
handed to callers. Nothing is visible unless that feature is enabled, and no plugin is fetched or
executed by rattler today.

| Part | State |
| --- | --- |
| `info.virtual_package_plugins` parsing (`repodata.json` and sharded index) | Implemented |
| `Gateway::virtual_package_plugins(channel, platform)` accessor | Implemented |
| Registrations on `RepoDataQueryOutput`, per channel subdir | Implemented |
| `rattler virtual-packages -c <channel>` for manual inspection | Implemented |
| Conflict resolution across channels | Deliberately not done -- reported as declared, caller decides |
| Plugin protocol types, execution, result caching | Not implemented |
| Solver injection, `CONDA_OVERRIDE_*`, lockfile representation | Not implemented |
| Trust / opt-in model | Open, blocks execution |
| prefix.dev upload validation | Not implemented (server side) |

`rattler_index` does not yet propagate the field: with the feature off it drops
`info.virtual_package_plugins` on a repodata round-trip, and with the feature on it writes an empty
map. Only a channel server publishing the field directly exercises the path today.

### Problem

Today, virtual packages like `__cuda` are hardcoded in the solver client. This made sense when NVIDIA
was the only accelerator that mattered, but the hardware landscape is diversifying fast. AMD ROCm,
Intel oneAPI, and other accelerator stacks each have their own driver versions, runtime libraries, and
capability matrices. Hardcoding detection logic for every new accelerator in every client release
doesn't scale. Channel operators who ship packages targeting these accelerators need a way to define
virtual packages like `__rocm` or `__oneapi` without waiting for upstream client changes.

### Proposal

We introduce a plugin-based virtual package system where channel operators on prefix.dev define custom
virtual packages backed by detection plugins. The solver treats them identically to built-in virtual
packages -- packages can depend on `__rocm >= 6.0` or `__oneapi >= 2025.1` the same way they depend on
`__cuda` today.

The system has two parts:

1. **Channel-side: plugin registration and validation**
2. **Client-side: plugin execution and caching**

---

### 1. Channel-Side: Plugin Registration

Channel operators register virtual package plugins as part of their channel configuration on
prefix.dev. Each registration names a conda package containing the detection logic and lists the
virtual packages that plugin provides.

During package upload, prefix.dev validates that any virtual package dependency declared in a
package's metadata has a corresponding plugin registered in the channel. Uploads referencing undefined
virtual packages are rejected.

The registration is published in the channel's `repodata.json` under a new `info.virtual_package_plugins`
field, keyed by **plugin package name**:

```json
{
  "info": {
    "virtual_package_plugins": {
      "cuda-detect": ["__cuda", "__cuda_arch"],
      "rocm-detect": ["__rocm"]
    }
  },
  "packages": { ... }
}
```

Keying by plugin rather than by virtual package is deliberate. The reverse direction --
`{"__cuda": "cuda-detect", "__cuda_arch": "cuda-detect"}` -- registers the same detector twice and
gives the client no way to know the two entries are one program doing one piece of work. Keying by
plugin makes "several virtual packages from one plugin" the ordinary case, which is what `__cuda` and
`__cuda_arch` actually need.

The client resolves the plugin package from the same channel, picking the latest available version. No
version constraint is expressible in the registration.

The `virtual_package -> plugin` mapping is *derived* by the client if it needs it. That inversion is
many-to-many: two plugins in one channel, or plugins in different channels, may each claim `__rocm`.
Nothing in the metadata prevents it and the client must resolve it.

The same field is published in the sharded repodata index (`repodata_shards.msgpack.zst`) under
`info`, so sharded channels carry the registration too.

**Per-subdir, not channel-wide.** `info` lives in each subdir's repodata, so the registration must be
repeated in every subdir of a channel, and different subdirs *may* declare different registrations.
Consumers see one entry per subdir and may union them. A channel-wide location would be better and
needs a CEP.

**Lenient parsing.** Plugin and virtual package names are parsed without validation, so a channel
publishing a malformed name does not make the whole `repodata.json` unusable.

### 2. Client-Side: Plugin Execution and Caching

*Not implemented. This section is the remaining proposal; the metadata above constrains it as noted.*

When pixi resolves an environment and encounters a dependency on a virtual package provided by a
registered plugin, it:

1. **Fetches and installs the plugin package** into an isolated, internal environment (separate from
   the user's env, cached across solves).

   The plugin environment must be solved using **built-in virtual packages only**. Resolving a
   plugin's own dependencies is itself a solve against a channel whose plugin data is not yet
   available; restricting that solve to built-ins is what stops the recursion.

2. **Executes the plugin once**, and the plugin reports on every virtual package it was registered
   for. It inspects the local system (checks for driver files, queries `rocm-smi`, reads `/sys/`
   entries, etc.) and returns a JSON array:

```json
{
  "virtual_packages": [
    { "name": "__cuda", "version": "12.4" },
    { "name": "__cuda_arch", "version": "0", "build_string": "sm_89" }
  ],
  "cache": {
    "ttl_seconds": 86400,
    "watch_paths": [
      "/opt/rocm/lib/libamdhip64.so",
      "/sys/module/amdgpu/version"
    ]
  }
}
```

   The array shape follows from one entry point per plugin package: a single `cuda-detect` run has to
   be able to report both `__cuda` and `__cuda_arch`. `build_string` is optional and exists because
   `__archspec` and `__cuda_arch` carry their information in the build string rather than the version;
   without it those cannot be expressed as plugins at all.

3. **Caches the result** according to the plugin's cache policy:
   - **`ttl_seconds`**: how long the cached value is valid.
   - **`watch_paths`**: file globs to monitor. If any file's existence or modification time changes,
     the cache is invalidated and the plugin re-runs. This handles driver upgrades between solves
     without requiring TTL expiry.

   Caches must be keyed on **(channel, plugin package name)**, not the package name alone: names are
   unique within a channel, but two channels may each ship a different `cuda-detect`.

4. **Injects the detected virtual packages** into the solver's virtual package set alongside the
   standard ones (`__cuda`, `__glibc`, etc.). A plugin may only inject virtual packages the channel
   registered for it; anything else is discarded. Virtual packages the plugin omits are treated as
   absent -- the solver simply won't select packages that require them.

### Plugin Interface

Plugins are simple executables. **The entry point is the plugin package name**: package `cuda-detect`
ships an executable `cuda-detect`. Package names are unique within a channel and a JSON object cannot
repeat a key, so the entry point needs no separate metadata field, and conda already puts executables
on the environment's `PATH` (`bin/`, `Scripts/`) so no path needs declaring either.

The contract:

- **stdin**: empty
- **stdout**: JSON object as shown above
- **stderr**: diagnostic output (logged by pixi at debug level)
- **exit 0**: the plugin ran; `virtual_packages` lists what it detected, and may be empty
- **exit non-zero**: plugin failure (pixi logs a warning, treats all of the plugin's virtual packages
  as absent)

This replaces the draft's earlier three-way exit code contract (`0` present / `1` absent / `2+`
failure). With several virtual packages per plugin, presence is per-entry in the output array and can
no longer be carried by a single exit status: `__cuda` may be present while `__cuda_arch` is not.
**This needs sign-off** -- it is the one part of the interface the implemented metadata forced to
change.

Plugins can be compiled binaries, shell scripts, or anything else that fits in a conda package.
Keeping the interface this simple means detection for a new accelerator is a single small package with
a shell script that checks a few paths.

### Gateway Integration

Implemented. The repodata gateway parses `info.virtual_package_plugins` and reports it; it does not
execute plugins -- it doesn't know what hardware the client has -- and it does not resolve conflicts.

There are two ways to read the registrations:

`Gateway::virtual_package_plugins(channel, platform)` returns the map for one subdirectory. It takes
no specs, which is the point: the plugin package names only exist inside the metadata being fetched,
so there is nothing to query for until it has been read. It mirrors `Gateway::channel_relations`,
reusing the internal subdir cache, and yields an empty map for a subdirectory that registers none or
does not exist.

`RepoDataQueryOutput::virtual_package_plugins` returns one entry per channel subdir that declared a
registration, carrying the channel, the subdir platform, and the plugin-to-virtual-packages map,
ordered by resolved channel priority (including any CEP-42 relation-derived ordering). This is the
view a solve sees, so it also covers channels discovered through CEP-42 that the caller never named.

Duplicate claims are preserved verbatim in both: two channels each claiming `__rocm`, or two plugins
within one channel each claiming `__rocm`, all come back, and no warning is raised. Deciding which
plugin wins is the caller's job.

For manual inspection, `rattler virtual-packages -c <channel>` prints the registrations a channel
declares. `test-data/channels/virtual-package-plugins` is a local fixture to point it at, since no
channel publishes the field yet.

All of this is behind the `experimental-virtual-package-plugins` feature. With the feature off the
gateway's public API and its serialized output are unchanged.

### Example: Supporting AMD ROCm

A channel operator shipping packages compiled against ROCm:

1. Creates a `rocm-detect` conda package containing a shell script named `rocm-detect` that checks for
   `/opt/rocm/.info/version` and parses the ROCm version.
2. Registers `rocm-detect -> ["__rocm"]` in their channel config on prefix.dev.
3. Uploads packages with `__rocm >= 6.0` in their run dependencies.
4. When a user with ROCm 6.1.2 installed runs `pixi install`, pixi fetches the plugin, runs it,
   discovers ROCm 6.1.2, and the solver selects the appropriate package variants.
5. A user without ROCm gets packages built for CPU fallback (or an unsatisfiable error if no fallback
   exists).

The same pattern works for Intel oneAPI, custom FPGA toolchains, or any other hardware capability that
packages need to select against.

### Example: One Plugin, Several Virtual Packages

A `cuda-detect` package registered as `cuda-detect -> ["__cuda", "__cuda_arch"]` queries the driver
once and reports both the driver version and the compute capability:

```json
{
  "virtual_packages": [
    { "name": "__cuda", "version": "12.4" },
    { "name": "__cuda_arch", "version": "0", "build_string": "sm_89" }
  ]
}
```

On a machine with no NVIDIA driver the same plugin exits 0 with an empty `virtual_packages` array.
Under the draft's original one-plugin-per-virtual-package scheme this needed two packages, or one
package with two entry points repeating the same driver query.

### Settled Decisions

1. **Registration is keyed by plugin package name**, mapping to the list of virtual packages it
   provides.
2. **The entry point is the plugin package name.** No entry-point field in the metadata; uniqueness
   within a channel comes for free.
3. **No package-record changes.** The registration lives entirely in `info`; `PackageRecord` and
   `index.json` are untouched, so a client learns what a plugin provides without fetching the plugin's
   record first.
4. **No version constraints in the registration.** Bare package name, latest version.
5. **The gateway reports, it does not decide.** Registrations come back per subdir in channel-priority
   order with duplicates intact.
6. **Plugin identity is (channel, package name)** for caching and conflict resolution.
7. **Everything is behind an experimental cargo feature** and invisible when it is off.

### Open Questions

1. **Trust and governance.** Execution runs channel-supplied code during solve. Users should have to
   opt in, and the shape of that opt-in is unsettled: a single global switch, or a per-channel
   allowlist. The nearest precedent in rattler is `run_post_link_scripts`, a two-state setting whose
   opt-in value is named `insecure` and which defaults to off. This blocks the executor.
2. **Cache invalidation of the plugin environment.** `ttl_seconds` and `watch_paths` cover the
   detection *result*. Setting up the isolated environment is the expensive part, and knowing when
   that environment is stale is a separate question.
3. **Cross-installing for another platform.** Detection is inherently host-only. Built-in virtual
   packages have `detect_for_platform` with documented cross-compilation defaults; plugins have no
   equivalent, and it is not clear what running a host plugin means when solving for a different
   target.
4. **Overrides and opt-out.** Users should be able to override or disable a specific virtual package
   (e.g. skip detection and assert `__rocm 6.1.2`). Built-ins use `CONDA_OVERRIDE_*`; the naming for
   plugin-provided packages, especially with sub-keys like `__cuda_arch` and with several channels
   registering the same name, is undecided.
5. **Reproducibility and lockfiles.** Whether the plugin version that produced a detection is recorded
   in the lock file, and when plugins are updated. Current leaning: always use the latest available and
   do not lock it, but this is unresolved.
6. **Channel-wide storage.** `info` is per-subdir, so the registration is duplicated across subdirs.
   A channel-wide metadata location would fix this and needs a CEP.
7. **Channel relations and overriding.** Whether a channel may register a plugin for a virtual package
   its base channel already covers (e.g. a private channel overriding `__glibc`), and whether such an
   override should affect the base channel. CEP-42 relations already give the gateway a priority
   order; the policy question is untouched.
8. **Plugin dependencies.** Detection plugins should be self-contained, but if one needs a shared
   library to query a driver API, those deps are resolved from the same channel. Solving the plugin
   environment with built-in virtual packages only (see above) breaks the bootstrap recursion; the
   remaining risk is ordinary dependency conflict.
9. **Versioning semantics.** Virtual package versions should follow conda version ordering so that
   constraints like `__rocm >= 6.0, < 7` work as expected.
10. **wheelnext.** Worth looking at closely -- they are solving essentially the same problem.
