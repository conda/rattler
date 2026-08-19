//! Snapshots of `MatchSpec` rendering in its awkward corners.
//!
//! The tests next door each assert one property of one spec. These snapshots
//! instead put the odd cases in one table, so a change in how any of them
//! renders shows up as a readable diff. Every case records both dialects and
//! what a reparse did with the text, because the interesting part is not the
//! string but whether it survives.
//!
//! The two dialects divide the work. Canonical output must always parse back
//! into the same spec (URLs modulo credential redaction) and render to the
//! same text again; anything it cannot express it refuses, and those refusals
//! are as much a part of the table as the strings. Legacy output may be text
//! the parser rejects, which is a loud and therefore acceptable failure. Where
//! it cannot even do that, the reparse line reads `DIVERGES` and the canonical
//! line below it reads `refused` — a pairing the report asserts, so no case
//! can quietly lose meaning in both dialects at once.

use std::{fmt::Write as _, path::PathBuf, sync::Arc};

use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, MatchSpecCondition, NamelessMatchSpec, PackageNameMatcher,
    ParseMatchSpecOptions, Platform, RepodataRevision, StringMatcher,
};
use url::Url;

/// Everything the grammar has to offer: the newest repodata revision, and
/// glob and regex package names.
fn options() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict()
        .with_repodata_revision(RepodataRevision::V3)
        .with_exact_names_only(false)
}

/// An empty root dir keeps local paths out of the snapshots; every channel
/// used here resolves against the channel alias instead.
fn channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(PathBuf::new())
}

fn named(name: &str) -> MatchSpec {
    MatchSpec {
        name: name.parse().unwrap(),
        ..MatchSpec::default()
    }
}

/// Wraps `leaf` in a `when=` condition on a spec named `target`.
fn when(leaf: MatchSpec) -> MatchSpec {
    MatchSpec {
        condition: Some(MatchSpecCondition::MatchSpec(Box::new(leaf))),
        ..named("target")
    }
}

/// Collapses a message onto one line so every case stays a fixed-shape block.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a reparse of the legacy rendering did. The legacy dialect is allowed
/// to render something the parser refuses, but it must never come back as a
/// different query. A reparse that renders to the very same text is the one
/// tolerated difference: the text carries everything it can, and the parser
/// reads it the only way it knows (`Exact("py*")` reads back as a glob, a
/// track feature holding a space reads back as two).
fn legacy_outcome(spec: &MatchSpec, rendered: &str) -> String {
    match MatchSpec::from_str(rendered, options()) {
        Ok(reparsed) if reparsed == *spec => "same spec".to_string(),
        Ok(reparsed) if reparsed.to_string() == rendered => {
            "differs, renders identically".to_string()
        }
        Ok(reparsed) => format!("DIVERGES: {reparsed}"),
        Err(error) => format!("rejected: {}", one_line(&error.to_string())),
    }
}

/// What a reparse of the canonical rendering did. Canonical output must parse,
/// come back as the same spec, and render to the same text again.
fn canonical_outcome(spec: &MatchSpec, rendered: &str) -> String {
    let reparsed = match MatchSpec::from_str(rendered, options()) {
        Ok(reparsed) => reparsed,
        Err(error) => return format!("REJECTED: {}", one_line(&error.to_string())),
    };
    let fidelity = if reparsed == *spec {
        "same spec"
    } else {
        "differs from input"
    };
    match reparsed.to_canonical_string() {
        Ok(second) if second == rendered => format!("{fidelity}, stable"),
        Ok(second) => format!("{fidelity}, UNSTABLE: {second}"),
        Err(error) => format!("{fidelity}, UNSTABLE: {}", one_line(&error.to_string())),
    }
}

/// Accumulates the report that becomes one snapshot.
#[derive(Default)]
struct Cases(String);

impl Cases {
    /// Records a spec built in code, described by `label`.
    fn add(&mut self, label: &str, spec: &MatchSpec) {
        let legacy = spec.to_string();
        let legacy_reparse = legacy_outcome(spec, &legacy);
        let canonical = spec.to_canonical_string();

        // The division of labour between the dialects, asserted rather than
        // only shown: what the legacy text cannot say faithfully, the
        // canonical dialect refuses outright.
        assert!(
            !legacy_reparse.starts_with("DIVERGES") || canonical.is_err(),
            "the legacy text for `{label}` describes a different query, yet canonical accepts it"
        );

        writeln!(self.0, "case      : {label}").unwrap();
        writeln!(self.0, "legacy    : {legacy}").unwrap();
        writeln!(self.0, "  reparse : {legacy_reparse}").unwrap();
        match canonical {
            Ok(canonical) => {
                let outcome = canonical_outcome(spec, &canonical);
                assert!(
                    !outcome.starts_with("REJECTED") && !outcome.contains("UNSTABLE"),
                    "canonical text for `{label}` is not a fixed point: {outcome}"
                );
                writeln!(self.0, "canonical : {canonical}").unwrap();
                writeln!(self.0, "  reparse : {outcome}").unwrap();
            }
            Err(error) => writeln!(self.0, "canonical : refused: {error}").unwrap(),
        }
        self.0.push('\n');
    }

    /// Records a spec parsed from `source`, which doubles as the label.
    fn parse(&mut self, source: &str) {
        match MatchSpec::from_str(source, options()) {
            Ok(spec) => self.add(source, &spec),
            Err(error) => {
                writeln!(self.0, "case      : {source}").unwrap();
                writeln!(
                    self.0,
                    "parse     : rejected: {}",
                    one_line(&error.to_string())
                )
                .unwrap();
                self.0.push('\n');
            }
        }
    }

    fn parse_all<'a>(&mut self, sources: impl IntoIterator<Item = &'a str>) {
        for source in sources {
            self.parse(source);
        }
    }
}

/// Which fields the legacy dialect keeps positional, and when it falls back to
/// the bracket form because a positional value would be re-tokenized.
#[test]
fn legacy_placement() {
    let mut cases = Cases::default();
    cases.parse_all([
        "foo",
        "foo >=1",
        "foo >=1,<2 py39h123_0",
        r#"foo[version=">=1,<2",build="py39h123_0"]"#,
        "foo 1.2.*",
        "foo * py39h123_0",
        r#"foo[build="py39h123_0",build_number=">=2"]"#,
        r#"foo[version="(>=1,<2)|>3"]"#,
        "conda-forge::foo >=1",
        "conda-forge/linux-64::foo",
        r#"foo[subdir="linux-64"]"#,
        r#"foo[fn="pkg.conda"]"#,
        r#"foo[namespace="python",version=">=1"]"#,
        r#"*[sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]"#,
        r#"foo[md5="0123456789abcdef0123456789abcdef",license="BSD-3-Clause",license_family="BSD"]"#,
    ]);

    // A build that carries a version-group separator, whitespace, or a
    // character an earlier tokenization stage eats cannot sit in the
    // positional slot.
    for build in ["a,b", "a|b", "a b", "a#b", "a[0]", "a;b", "a:b"] {
        let spec = MatchSpec {
            build: Some(StringMatcher::Exact(build.to_string())),
            ..named("foo")
        };
        cases.add(&format!("build = {build}"), &spec);
    }

    insta::assert_snapshot!(cases.0);
}

/// Name matchers keep their glob/regex classification through a round-trip, or
/// canonical refuses them.
#[test]
fn package_names() {
    let mut cases = Cases::default();
    cases.parse_all([
        "foo", "foo*", "*", "^foo.*$",
        // `and` and `or` are only special inside a condition, and `pandas`
        // merely contains one of them.
        "and", "pandas", "__linux",
    ]);

    // Matchers that no name text can express: the rendering would reparse as a
    // different matcher, or end the name early.
    for name in ["^py(thon|py)[0-9]$", "^foo$bar$", "py*:3", "py* 3"] {
        let Ok(matcher) = name.parse::<PackageNameMatcher>() else {
            continue;
        };
        let spec = MatchSpec {
            name: matcher,
            ..MatchSpec::default()
        };
        cases.add(&format!("name = {name}"), &spec);
    }

    // A programmatically built exact matcher whose text classifies as a glob
    // has no faithful rendering either.
    cases.add(
        "build = Exact(\"py*\") (renders as a glob)",
        &MatchSpec {
            build: Some(StringMatcher::Exact("py*".to_string())),
            ..named("foo")
        },
    );

    insta::assert_snapshot!(cases.0);
}

/// Quoting: the parser stores most quoted scalars verbatim, so the delimiter is
/// picked to fit the value rather than the value being escaped.
#[test]
fn scalar_quoting() {
    let mut cases = Cases::default();
    for (label, value) in [
        ("plain", "pkg.conda"),
        ("single quote", "it's-here.conda"),
        ("double quote", r#"say-"hi".conda"#),
        ("escaped single quotes", r#"one \'single\' and "double""#),
        ("escaped double quotes", r#"one \"double\" and 'single'"#),
        ("both delimiters unescaped", r#"both ' and " quotes"#),
        ("trailing backslash", r"C:\cache\"),
        ("even backslash run", r"C:\cache\\"),
        ("whitespace", "has space.conda"),
        ("comment character", "hash#comment.conda"),
        ("semicolon", "semi;colon.conda"),
        ("brackets", "bracket[0].conda"),
        ("comma", "comma,list.conda"),
        ("equals sign", "equals=sign.conda"),
        ("bracket field lookalike", "x],version=[2"),
        ("empty", ""),
        ("non-ascii", "\u{1f980}-\u{3b1}\u{3b2}.conda"),
    ] {
        let spec = MatchSpec {
            file_name: Some(value.to_string()),
            ..named("foo")
        };
        cases.add(&format!("fn ({label}) = {value}"), &spec);
    }

    insta::assert_snapshot!(cases.0);
}

/// Channels render by name only when the name reconstructs them; everything
/// else falls back to the base URL, and canonical never emits credentials.
#[test]
fn channels() {
    let mut cases = Cases::default();
    cases.parse_all([
        "conda-forge::foo",
        "https://repo.example/custom::foo",
        "https://repo.example/custom[linux-64,noarch]::foo",
        "conda-forge[noarch]::foo",
    ]);

    let with_channel = |channel: Channel| MatchSpec {
        channel: Some(Arc::new(channel)),
        ..named("foo")
    };

    // A name whose last segment is a platform would be split into channel and
    // subdir on reparse, so it renders as a URL instead.
    cases.add(
        "channel named conda-forge/linux-64",
        &with_channel(Channel::from_str("conda-forge/linux-64", &channel_config()).unwrap()),
    );

    // Host brackets must not be mistaken for a platform selector.
    cases.add(
        "ipv6 host with platform selector",
        &with_channel(
            Channel::from_str("https://[::1]:8080/channel[linux-64]", &channel_config()).unwrap(),
        ),
    );

    // An explicit empty selector is indistinguishable from an omitted one.
    cases.add(
        "explicit empty platform selector",
        &with_channel(
            Channel::from_str("https://repo.example/custom", &channel_config())
                .unwrap()
                .with_explicit_platforms([] as [Platform; 0]),
        ),
    );

    // Legacy renders URLs as they are; canonical strips credentials, which is
    // the one case where a canonical round-trip does not return an equal spec.
    let secret = Url::parse(
        "https://user:password@prefix.dev/t/path-token/channel/pkg.conda?auth=session#ticket=fragment-token",
    )
    .unwrap();
    cases.add(
        "channel and url carrying credentials",
        &MatchSpec {
            url: Some(secret.clone()),
            channel: Some(Arc::new(Channel::from_url(secret))),
            ..named("foo")
        },
    );

    cases.parse_all([
        r#"foo[url="https://repo.example/pkg.conda#sha256:deadbeef"]"#,
        r#"foo[url="https://repo.example/pkg.conda#md5:0123456789abcdef0123456789abcdef"]"#,
    ]);

    insta::assert_snapshot!(cases.0);
}

/// Conditions only get the parentheses their structure needs, and leaves that
/// the condition tokenizer would read as an operator are refused.
#[test]
fn conditions() {
    let mut cases = Cases::default();
    cases.parse_all([
        r#"target[when="a"]"#,
        r#"target[when="a and b"]"#,
        r#"target[when="a and b and c"]"#,
        r#"target[when="a and b or c"]"#,
        r#"target[when="a or b and c"]"#,
        r#"target[when="(a or b) and c"]"#,
        r#"target[when="a and (b and c)"]"#,
        r#"target[when="(a or b) or c"]"#,
        r#"target[when="a or (b or c)"]"#,
        r#"target[when="((a and b) or (c and d)) and e"]"#,
        r#"target[when="python >=3.11 and __linux"]"#,
        r#"target[when="python 1.2.*"]"#,
        r#"target[when="python[extras=[docs],flags=[cuda]] and __linux"]"#,
        r#"target[when="pandas >=2"]"#,
    ]);

    // A leaf that tokenizes as an operator, groups, or another condition
    // cannot be written back out.
    for leaf in ["and", "or"] {
        cases.add(&format!("condition leaf named {leaf}"), &when(named(leaf)));
    }
    cases.add(
        "condition leaf with a grouped regex name",
        &when(MatchSpec {
            name: "^(py|c)python$".parse().unwrap(),
            ..MatchSpec::default()
        }),
    );
    cases.add(
        "nested when in a condition leaf",
        &when(when(named("__linux"))),
    );

    insta::assert_snapshot!(cases.0);
}

/// List fields quote elements the grammar would otherwise split, and canonical
/// refuses the ones no element text can express.
#[test]
fn list_fields() {
    let mut cases = Cases::default();
    cases.parse_all([
        r#"foo[extras=[docs]]"#,
        r#"foo[extras=[docs,tests]]"#,
        r#"foo[flags=[cuda,mkl]]"#,
        r#"foo[track_features="mkl debug"]"#,
    ]);

    for extras in [vec![], vec![String::new()], vec!["docs,tests".to_string()]] {
        let spec = MatchSpec {
            extras: Some(extras.clone()),
            ..named("foo")
        };
        cases.add(&format!("extras = {extras:?}"), &spec);
    }

    for flag in ["cuda*", "", "with space"] {
        let spec = MatchSpec {
            flags: Some(vec![StringMatcher::Exact(flag.to_string())]),
            ..named("foo")
        };
        cases.add(&format!("flags = [Exact({flag:?})]"), &spec);
    }

    for track_features in [
        vec![],
        vec![String::new()],
        vec!["mkl debug".to_string()],
        vec!["mkl,debug".to_string()],
    ] {
        let spec = MatchSpec {
            track_features: Some(track_features.clone()),
            ..named("foo")
        };
        cases.add(&format!("track_features = {track_features:?}"), &spec);
    }

    insta::assert_snapshot!(cases.0);
}

/// Inputs sitting on the edge of the bracket grammar. Most are refused; the
/// point of the snapshot is that none of them is quietly read as something
/// else.
#[test]
fn parser_edges() {
    let mut cases = Cases::default();
    cases.parse_all([
        // A `]` inside a quoted value does not close the section.
        r#"foo[fn="]"]"#,
        "foo]",
        "foo[",
        "foo[]",
        "foo[unknown=1]",
        // A malformed platform selector is not a literal channel name.
        "conda-forge[linux-64]suffix::foo",
        "foo # comment",
        r#"foo[build="py39"] # comment"#,
        "foo;",
        "https://[::1]:8080/channel[linux-64]::foo",
        // The group separator would pull the build back into the version.
        "foo ==1 ,*",
    ]);

    insta::assert_snapshot!(cases.0);
}

/// A nameless spec renders every field it has, so nothing is dropped when the
/// name is supplied elsewhere.
#[test]
fn nameless_specs() {
    let mut report = String::new();
    for source in [
        "",
        "*",
        ">=1,<2",
        ">=1 py39h123_0",
        r#"[extras=[docs]]"#,
        r#"[flags=[cuda]]"#,
        r#"[when="python >=3.11"]"#,
        r#"[track_features="mkl"]"#,
        r#"[subdir="linux-64",license="BSD-3-Clause"]"#,
    ] {
        let spec = match NamelessMatchSpec::from_str(source, options()) {
            Ok(spec) => spec,
            Err(error) => {
                writeln!(report, "case      : {source}").unwrap();
                writeln!(
                    report,
                    "parse     : rejected: {}",
                    one_line(&error.to_string())
                )
                .unwrap();
                report.push('\n');
                continue;
            }
        };
        let rendered = spec.to_string();
        // A spec with no fields at all renders as the historic `*`
        // placeholder, which reads back as `version: Any` and so matches the
        // same packages.
        let outcome = match NamelessMatchSpec::from_str(&rendered, options()) {
            Ok(reparsed) if reparsed == spec => "same spec".to_string(),
            Ok(reparsed) if reparsed.to_string() == rendered => {
                "differs, renders identically".to_string()
            }
            Ok(reparsed) => format!("DIVERGES: {reparsed}"),
            Err(error) => format!("rejected: {}", one_line(&error.to_string())),
        };
        writeln!(report, "case      : {source}").unwrap();
        writeln!(report, "display   : {rendered}").unwrap();
        writeln!(report, "  reparse : {outcome}").unwrap();
        report.push('\n');
    }

    insta::assert_snapshot!(report);
}
