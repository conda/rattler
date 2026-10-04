# Changelog

All notable changes to js-rattler will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-08-19

### Highlights

**The gateway is reachable from JavaScript.** `Gateway` existed in the sources before but was never re-exported, so it was absent from the published package. It is exported now, with `query` for records, `names` for package names, and `channelNotices` for CEP-6 notices. Records come back as plain repodata JSON extended with `fn`, `url` and `channel`.

```js
const gateway = new Gateway();
const records = await gateway.query(
    ["https://prefix.dev/conda-forge"],
    ["linux-64", "noarch"],
    ["python 3.13.*"],
);
console.log(records.length, records[0].fn);
```

**Bring your own `fetch`.** Every request the gateway makes goes through the `fetch` option when you set one, so you can route repodata through your own HTTP stack: auth, a proxy, a cache, or a stub in tests. Without it the global `fetch` is used, which is what you want in browsers and plain Node. Non-fatal warnings go to the `onWarning` callback, or to `console.warn` when you don't pass one.

```js
const gateway = new Gateway({
    fetch: (request) => myAuthenticatedFetch(request),
    onWarning: (message) => log.warn(message),
});
```

**Errors are `Error` objects now, and they carry a code.** Until 0.3.5 the bindings threw bare strings, so `err.message` was `undefined` and `err instanceof Error` was `false`. Every failure is a real `Error` with a stable `code`, and `isRattlerError` narrows it in TypeScript.

```js
try {
    new Version("!!not a version!!");
} catch (err) {
    if (isRattlerError(err)) console.log(err.code); // "PARSE_VERSION"
}
```

**Repodata v3 match spec syntax.** `simpleSolve` parses extras, conditionals and flags, which were behind an experimental feature flag before and were rejected outright: `python[extras=[foo]]`, `python[when="numpy"]`, `python[flags=[cuda]]`. `PackageRecord.flags` and `SolvedPackage.flags` expose the flags themselves.

**Solving got faster.** `resolvo` went from 0.10.1 to 0.12.0 across this release, with a reworked clause encoding and decision queue. The published numbers are from the native benchmarks, so treat them as a direction rather than a promise for WASM. Nothing to change on your side.

**Breaking: `simpleSolve` follows CEP-42 channel relations.** A channel that declares `channel_relations` now pulls its related channels into the query and into the priority order, where they used to be ignored. A solve against such a channel can return a different set of packages than it did on 0.3.5. Only channels publishing CEP-42 metadata are affected, and problems with the metadata surface through `onWarning` rather than failing the solve ([#2462](https://github.com/conda/rattler/pull/2462)).

**Breaking: `loong64` is spelled `loongarch64`.** The platform is `"linux-loongarch64"` and the arch is `"loongarch64"`. The old spellings are gone from `platformNames`, `archNames`, the `Platform` and `Arch` unions, and the Rust parser, so `platformArch("linux-loong64")` returns `undefined` and passing `"linux-loong64"` to `simpleSolve` throws `PARSE_PLATFORM` ([#1957](https://github.com/conda/rattler/pull/1957)).

```js
// before
platformArch("linux-loong64"); // "loong64"
// now
platformArch("linux-loongarch64"); // "loongarch64"
```

### Added

- Export `Gateway`, with `query`, `names` and `channelNotices` in [#1930](https://github.com/conda/rattler/pull/1930), [#2687](https://github.com/conda/rattler/pull/2687) and [#2639](https://github.com/conda/rattler/pull/2639)
- `fetch` and `onWarning` options on `Gateway`, plus the `warnings` the gateway collected on a `query` result in [#2687](https://github.com/conda/rattler/pull/2687) and [#2688](https://github.com/conda/rattler/pull/2688)
- `isRattlerError`, `RattlerError` and the `RattlerErrorCode` union in [#2688](https://github.com/conda/rattler/pull/2688)
- `flags` on `PackageRecord` and on the records `simpleSolve` returns, for simplified variant selection in [#2381](https://github.com/conda/rattler/pull/2381)
- Extras, conditionals and `flags` in the match specs `simpleSolve` accepts, graduated from experimental in [#2450](https://github.com/conda/rattler/pull/2450)
- `emscripten-wasm64` platform and `wasm64` arch in [#2680](https://github.com/conda/rattler/pull/2680) and [#2694](https://github.com/conda/rattler/pull/2694)
- `freebsd-32` and `freebsd-arm64` platforms in [#2227](https://github.com/conda/rattler/pull/2227)

### Changed

- **BREAKING:** `simpleSolve` follows a channel's CEP-42 `channel_relations`, so channels declaring relations contribute extra channels to the solve and to the priority order in [#2462](https://github.com/conda/rattler/pull/2462)
- **BREAKING:** `linux-loong64` is `linux-loongarch64` and the `loong64` arch is `loongarch64`, in `platformNames`, `archNames`, `platformArch` and the `Platform` and `Arch` types in [#1957](https://github.com/conda/rattler/pull/1957)
- Errors thrown by the bindings are `Error` instances carrying a `code` instead of bare strings in [#2688](https://github.com/conda/rattler/pull/2688)
- Bump `resolvo` from 0.10.1 to 0.12.0 in [#2520](https://github.com/conda/rattler/pull/2520) and [#2609](https://github.com/conda/rattler/pull/2609)
- Rate a solvable's repeated requirements on the same package by the most restrictive one, so `simpleSolve` picks the same build for tied candidates across platforms in [#2649](https://github.com/conda/rattler/pull/2649)
- Abbreviate long version lists in the message of an unsolvable `simpleSolve` in [#2481](https://github.com/conda/rattler/pull/2481)

### Fixed

- Match build strings case-insensitively per CEP-29, so `python * H_C8DE616_6_CP313` resolves the same package as the lowercase spelling in [#2386](https://github.com/conda/rattler/pull/2386)
- Keep a trailing underscore in a version instead of splitting it off, per CEP 33: `new VersionSpec("1.0_")` parses to `==1.0_` where it used to throw in [#2606](https://github.com/conda/rattler/pull/2606)
- Match an extra trailing component in `Version.startsWith`: `new Version("1.0.0_version1").startsWith(new Version("1.0.0_version"))` is `true` in [#1920](https://github.com/conda/rattler/pull/1920) and [#1791](https://github.com/conda/rattler/pull/1791)
- Treat `dev` and `post` as special only for exact component runs, which changes how `Version.compare` orders `1.2devdev` against `1.2dev` and makes `new Version("1.2devdev").isDev` `false` in [#2299](https://github.com/conda/rattler/pull/2299)
- Accept quoted extras lists (`foobar[extras=["science"]]`) and enforce CEP 44's group-name grammar in [#2552](https://github.com/conda/rattler/pull/2552)

### Performance

- Speed up version, version spec and match spec parsing in [#2515](https://github.com/conda/rattler/pull/2515) and [#2066](https://github.com/conda/rattler/pull/2066)

[Unreleased]: https://github.com/conda/rattler/compare/js-rattler-v0.4.0...HEAD
[0.4.0]: https://github.com/conda/rattler/compare/js-rattler-v0.3.5...js-rattler-v0.4.0
