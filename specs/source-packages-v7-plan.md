# Source Package Format v7 – Implementation Plan

## Assignment Overview
- bump the rattler-lock format to **v7** with a breaking change that trims source package metadata down to `name`, `variants`, dependencies, license, purls, sources, input hashes, build-source info, and python site path.
- ensure `CondaSourceData::variants` becomes a mandatory (possibly empty) `BTreeMap<String, VariantValue>`; older v6 readers populate it with an empty map.
- rework package disambiguation so environments distinguish source packages using only `name + variants`; serialization must fail if that does not uniquely identify packages.
- drop the blanket `Matches<MatchSpec>` implementation on `CondaPackageData`; keep it only for binaries because sources no longer expose full `PackageRecord`s.
- accept that v6 files cannot be losslessly re-serialized as v7 without user-provided variants.
- surface the parsed lock-file version alongside the loaded data so consumers can detect legacy files immediately.

## Known Challenges
- **API ripple:** `CondaSourceData` currently exposes a full `PackageRecord`; removing it touches builders, ordering logic, merge helpers, Python/JS bindings, and downstream consumers that call `.record()`.
- **Deduplication rewrite:** `UniqueCondaIdentifier` and environment serialization rely on version/build; moving to variants requires updated hashing, equality, and sort ordering.
- **Serde compatibility:** v6 parsing must hydrate an empty variants map and still honour legacy dedup/state when serializing v6 output; we need clear errors when attempting to emit v7 without variants.
- **Trait removal:** detaching `Matches` from source packages affects any code that used `CondaPackageData::matches`; we must audit public API surface (Rust, Python, JS) for replacements.
- **Ambiguity detection:** we must prove that `{name, variants}` is sufficient; errors should surface with actionable diagnostics when duplicates remain.
- **Selector version split:** `parse_from_document_v*` is generic over version markers, but `DeserializablePackageSelector` is monolithic today. Changing selector semantics for v7 likely means introducing separate selector enums per version and threading them through the generic to keep compatibility clean.

## Design Considerations
- `VariantValue` should stay a serde-untagged enum (String/i64/bool) with deterministic ordering for maps; empty map represents “no pinned variants”.
- Introduce helper accessors on `CondaPackageData` for common fields (`name()`, `variants()`, `depends()`), returning borrowed data without cloning.
- For serialization, binary packages keep existing disambiguation (name/version/build/subdir); sources rely on `variants`.  When conflict remains, bubble up a descriptive serialization error.
- Dedup keys: binaries continue to use `(location, name-normalized, version, build, subdir)`; sources use `(location, name-normalized, variants)`; avoid mixing the two to keep hashing predictable.
- Retain v6 parsing/serialization through dedicated model structs; when deserializing v6 sources, set `variants = BTreeMap::new()`.  When writing v6, fall back to legacy fields but never emit variants because readers ignore them.
- Update public bindings (py-rattler/js) to surface the reduced source-data view and document the breaking change.

## Step-by-Step Plan
Each milestone below should land with targeted unit tests (or focused integration tests when more appropriate) that lock in the behaviour being introduced or changed.

0. **Parser groundwork**
   - Refactor `parse_from_lock` to reduce nesting (extract helpers for URL indexing, selector resolution, and environment assembly) while preserving existing behaviour.
   - Introduce version-specific selector structs (e.g. `SelectorV5`, `SelectorV6`) and make `DeserializableEnvironment` generic over the selector type, with a `resolve(&self, …) -> Result<EnvironmentPackageData>` helper.
   - Add regression/unit tests to cover the new helpers and ensure current v5/v6 fixtures still parse identically.
   - Adjust parsing entrypoints (`parse_from_document_v*`, `LockFile::from_*`) to return both the `LockFile` and the detected `FileFormatVersion`, and add smoke tests ensuring the version surfaces correctly for v5/v6 fixtures.

1. **Version bump scaffolding**
   - Extend `FileFormatVersion` enum with `V7`, set `LATEST`, update parser switch, adjust tests expecting max version.
   - Add changelog entry announcing the breaking change.
2. **Data structure overhaul**
   - Define `VariantValue` (if not already present) in a shared module; implement `Ord`, `Serialize`, `Deserialize`.
   - Refactor `CondaSourceData` to store the minimal fields with `variants: BTreeMap<_, _>` defaulting to empty.
   - Update constructors, `merge`, and helper methods; guard `.record()` to return `Some` only for binaries (or remove; only expose new getters).
3. **Trait split**
   - Remove `impl Matches<MatchSpec> for CondaPackageData`; reintroduce an implementation for `CondaBinaryData` alone; provide forwarding helper `CondaPackageData::satisfies` that delegates only for binaries.
   - Add focused unit tests covering binary `Matches` behaviour and asserting that sources opt out (e.g. panic/error paths).
4. **Builder & dedup logic**
   - Rewrite `UniqueCondaIdentifier` as an enum (Binary/Source) with appropriate keys.
   - Adjust `Ord`, `PartialOrd`, and map storage to use new identifiers.
   - Ensure builder merge path handles source variants correctly and updates environment package indexes.
5. **Serialization updates**
   - Modify `SerializablePackageSelector` logic: sources can add `variants` field instead of `version/build`; update comparator ordering.
   - Introduce ambiguity detection error if `(name, variants)` filters still leave >1 candidate.
   - Adjust source package serialization to include only the allowed fields.
   - Add unit tests for selector emission, ordering, and ambiguity failures.
6. **Deserialization paths**
   - Add a `SelectorV7` implementation that deserializes variant-based selectors and plugs into the previously refactored `parse_from_lock`.
   - Extend v6/v7 model structs to map YAML into new `CondaSourceData`.
   - When reading v6, set `variants = BTreeMap::new()`; when reading v7, require the field.
   - Update environment package selection to match on variants when supplied.
   - Cover v6 and v7 selector variants with unit tests to catch regression in match logic.
7. **Bindings & API consumers**
   - Update Python (`py-rattler`) and JS bindings to reflect new source fields and lack of `PackageRecord`.
   - Provide helper methods in FFI to fetch variants, depends, etc.
8. **Testing & docs**
   - Refresh snapshots/fixtures, including ambiguity failure cases.
   - Add regression tests ensuring v6→v7 conversion errors without variants.
   - Document migration guidance in `CHANGELOG.md` and developer docs.
