//! Round-trip property tests for the unified [`MatchSpec`] renderer
//! (`src/match_spec/format.rs`), distilled from an adversarial review. Each
//! test builds a systematic matrix of specs or condition ASTs, renders them
//! through `Display` / `to_canonical_string`, reparses, and reports every
//! divergence or panic with the offending input.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, MatchSpecCondition, ParseMatchSpecOptions, ParseStrictness,
    RepodataRevision, StringMatcher, VersionSpec,
};
use url::Url;

fn strict_v3() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict()
        .with_repodata_revision(RepodataRevision::V3)
        .with_exact_names_only(false)
}

fn channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::temp_dir())
}

fn mk_channel(s: &str) -> Arc<Channel> {
    Arc::new(Channel::from_str(s, &channel_config()).unwrap())
}

fn version(s: &str) -> VersionSpec {
    VersionSpec::from_str(s, ParseStrictness::Strict)
        .unwrap_or_else(|e| panic!("test-matrix version {s:?} failed to parse: {e}"))
}

fn base(name: &str) -> MatchSpec {
    MatchSpec {
        name: name.parse().unwrap(),
        ..MatchSpec::default()
    }
}

/// Compares two [`MatchSpec`]s field by field and returns the names of fields
/// that differ.
fn spec_diff(a: &MatchSpec, b: &MatchSpec) -> Vec<&'static str> {
    let mut diffs = Vec::new();
    if a.name != b.name {
        diffs.push("name");
    }
    if a.version != b.version {
        diffs.push("version");
    }
    if a.build != b.build {
        diffs.push("build");
    }
    if a.build_number != b.build_number {
        diffs.push("build_number");
    }
    if a.file_name != b.file_name {
        diffs.push("file_name");
    }
    if a.extras != b.extras {
        diffs.push("extras");
    }
    if a.flags != b.flags {
        diffs.push("flags");
    }
    if a.channel != b.channel {
        diffs.push("channel");
    }
    if a.subdir != b.subdir {
        diffs.push("subdir");
    }
    if a.namespace != b.namespace {
        diffs.push("namespace");
    }
    if a.md5 != b.md5 {
        diffs.push("md5");
    }
    if a.sha256 != b.sha256 {
        diffs.push("sha256");
    }
    if a.url != b.url {
        diffs.push("url");
    }
    if a.license != b.license {
        diffs.push("license");
    }
    if a.license_family != b.license_family {
        diffs.push("license_family");
    }
    if a.condition != b.condition {
        diffs.push("condition");
    }
    if a.track_features != b.track_features {
        diffs.push("track_features");
    }
    diffs
}

/// True when the specs only differ in the URL, and the original URL carried
/// data the canonical form documents as redacted.
fn url_redaction_only(orig: &MatchSpec, reparsed: &MatchSpec) -> bool {
    let Some(url) = orig.url.as_ref() else {
        return false;
    };
    let sensitive = !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url
            .fragment()
            .is_some_and(|f| !f.starts_with("sha256:") && !f.starts_with("md5:"))
        || url.path().contains("/t/");
    if !sensitive {
        return false;
    }
    let mut patched = reparsed.clone();
    patched.url = orig.url.clone();
    patched == *orig
}

/// True when the specs only differ in the channel, and its base URL carried
/// credentials the canonical form documents as redacted.
fn channel_redaction_only(orig: &MatchSpec, reparsed: &MatchSpec) -> bool {
    let Some(channel) = orig.channel.as_deref() else {
        return false;
    };
    let url = channel.base_url.url();
    let sensitive = !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.path().contains("/t/");
    if !sensitive {
        return false;
    }
    let mut patched = reparsed.clone();
    patched.channel = orig.channel.clone();
    patched == *orig
}

/// The systematic matrix of (label, spec) cases used by the round-trip
/// tests.
fn spec_matrix() -> Vec<(String, MatchSpec)> {
    let o = strict_v3();
    let mut specs: Vec<(String, MatchSpec)> = Vec::new();
    let mut push = |label: String, spec: MatchSpec| specs.push((label, spec));

    let names = ["python", "py*", "*", "^py.*$", "^py(?!py).*$"];
    let versions = [
        "*",
        "1.2.*",
        ">=1.2",
        ">=1,<2|==3",
        "==1.0.0",
        "~=1.2",
        "1!2.0a0.*",
        "!=2.0",
        ">=3.6|==2.7",
    ];
    let builds = ["py37_0", "py*", "^py.*$", "*", "0"];

    // Every name alone.
    for name in names {
        push(format!("name={name}"), base(name));
    }

    // name x version x build.
    for name in names {
        for v in versions {
            for b in [None, Some("py*"), Some("py37_0")] {
                let mut spec = base(name);
                spec.version = Some(version(v));
                spec.build = b.map(|b| b.parse().unwrap());
                push(format!("name={name} version={v} build={b:?}"), spec);
            }
        }
    }
    // Build without version.
    for b in builds {
        let mut spec = base("python");
        spec.build = Some(b.parse().unwrap());
        push(format!("build-only={b}"), spec);
    }

    // Weird string values for each free-text field.
    let weird = [
        "plain",
        "sp ace",
        "qu\"ote",
        "back\\\\slash",
        "comma,val",
        "brack[et]",
        "closer]v",
        "hash#tag",
        "\u{fc}n\u{ef}-c\u{f8}d\u{e9}",
        "'single'",
        "a and b",
        "a or b",
        "*star*",
        "equals=sign",
        "semi;colon",
    ];
    for w in weird {
        let mut spec = base("python");
        spec.file_name = Some(w.to_string());
        push(format!("file_name={w:?}"), spec);

        let mut spec = base("python");
        spec.license = Some(w.to_string());
        push(format!("license={w:?}"), spec);

        let mut spec = base("python");
        spec.license_family = Some(w.to_string());
        push(format!("license_family={w:?}"), spec);

        let mut spec = base("python");
        spec.subdir = Some(w.to_string());
        spec.channel = Some(mk_channel("conda-forge"));
        push(format!("subdir={w:?}"), spec);

        let mut spec = base("python");
        spec.namespace = Some(w.to_string());
        push(format!("namespace={w:?}"), spec);
    }

    // Extras / flags / track_features.
    for extras in [vec!["docs"], vec!["docs", "tests"], vec!["a-b_c"]] {
        let mut spec = base("python");
        spec.extras = Some(extras.iter().map(ToString::to_string).collect());
        push(format!("extras={extras:?}"), spec);
    }
    for flags in [vec!["cuda"], vec!["cuda", "avx2"]] {
        let mut spec = base("python");
        spec.flags = Some(flags.iter().map(|f| f.parse().unwrap()).collect());
        push(format!("flags={flags:?}"), spec);
    }
    for tf in [vec!["mkl"], vec!["mkl", "debug"]] {
        let mut spec = base("python");
        spec.track_features = Some(tf.iter().map(ToString::to_string).collect());
        push(format!("track_features={tf:?}"), spec);
    }

    // Hashes / build number / fn / url from parsed template.
    let template = MatchSpec::from_str(
        r#"python[md5="0123456789abcdef0123456789abcdef",sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",build_number=">=2",fn="pkg.conda"]"#,
        o,
    )
    .unwrap();
    push("md5+sha256+build_number+fn".to_string(), template.clone());

    for url in [
        "https://repo.example/pkg.conda",
        "https://user:password@repo.example/pkg.conda",
        "https://repo.example/pkg.conda?query=1",
        "https://repo.example/pkg.conda#sha256:deadbeef",
        "https://repo.example/t/secret-token/pkg.conda",
        "file:///C:/pkg%20dir/x.conda",
    ] {
        let mut spec = base("python");
        spec.url = Some(Url::parse(url).unwrap());
        push(format!("url={url}"), spec);
    }

    // Channels.
    for channel in [
        "conda-forge",
        "https://repo.example/custom",
        "https://repo.example/custom[linux-64,noarch]",
        "https://user:pass@repo.example/custom",
        "*",
    ] {
        let mut spec = base("python");
        spec.channel = Some(mk_channel(channel));
        push(format!("channel={channel}"), spec);

        let mut spec = base("python");
        spec.channel = Some(mk_channel(channel));
        spec.subdir = Some("linux-64".to_string());
        push(format!("channel={channel}+subdir"), spec);
    }

    // subdir without channel; namespace positional.
    let mut spec = base("python");
    spec.subdir = Some("linux-64".to_string());
    push("subdir-without-channel".to_string(), spec);

    let mut spec = base("python");
    spec.namespace = Some("ns".to_string());
    push("namespace-only".to_string(), spec);

    let mut spec = base("python");
    spec.namespace = Some("ns".to_string());
    spec.channel = Some(mk_channel("conda-forge"));
    push("namespace+channel".to_string(), spec);

    // Simple condition.
    let mut spec = base("target");
    spec.condition = Some(MatchSpecCondition::MatchSpec(Box::new(base("__linux"))));
    push("simple-condition".to_string(), spec);

    // Everything at once for each name flavour.
    for name in names {
        let mut spec = template.clone();
        spec.name = name.parse().unwrap();
        spec.version = Some(version(">=1,<2|==3"));
        spec.build = Some("py*".parse().unwrap());
        spec.extras = Some(vec!["docs".to_string()]);
        spec.flags = Some(vec!["cuda".parse().unwrap()]);
        spec.channel = Some(mk_channel("https://repo.example/custom[linux-64,noarch]"));
        spec.subdir = Some("linux-64".to_string());
        spec.namespace = Some("python".to_string());
        spec.url = Some(Url::parse("https://repo.example/pkg.conda#sha256:deadbeef").unwrap());
        spec.license = Some("BSD-3-Clause".to_string());
        spec.license_family = Some("BSD".to_string());
        spec.track_features = Some(vec!["mkl".to_string(), "debug".to_string()]);
        spec.condition = Some(MatchSpecCondition::And(
            Box::new(MatchSpecCondition::MatchSpec(Box::new(base("__linux")))),
            Box::new(MatchSpecCondition::MatchSpec(Box::new({
                let mut leaf = base("python");
                leaf.version = Some(version(">=3.6"));
                leaf
            }))),
        ));
        push(format!("kitchen-sink name={name}"), spec);
    }

    specs
}

/// Every canonical `Ok` must reparse (strict V3) to an equal spec, modulo
/// the documented URL credential redaction, and must be idempotent.
#[test]
fn canonical_roundtrip_matrix() {
    let options = strict_v3();
    let mut failures = Vec::new();
    let mut ok_count = 0;
    for (label, spec) in spec_matrix() {
        let canonical = match catch_unwind(AssertUnwindSafe(|| spec.to_canonical_string())) {
            Ok(Ok(canonical)) => canonical,
            Ok(Err(_error)) => continue, // Err is allowed; wrong Oks are the hunt.
            Err(_) => {
                failures.push(format!("[PANIC in to_canonical_string] {label}"));
                continue;
            }
        };
        ok_count += 1;
        let reparsed = match MatchSpec::from_str(&canonical, options) {
            Ok(reparsed) => reparsed,
            Err(error) => {
                failures.push(format!(
                    "[canonical Ok but unparseable] {label}\n    canonical: {canonical}\n    error: {error}"
                ));
                continue;
            }
        };
        if reparsed != spec
            && !url_redaction_only(&spec, &reparsed)
            && !channel_redaction_only(&spec, &reparsed)
        {
            failures.push(format!(
                "[canonical WRONG Ok] {label}\n    canonical: {canonical}\n    fields: {:?}",
                spec_diff(&spec, &reparsed)
            ));
        }
        // Idempotence.
        match catch_unwind(AssertUnwindSafe(|| reparsed.to_canonical_string())) {
            Ok(Ok(canonical2)) => {
                if canonical2 != canonical {
                    failures.push(format!(
                        "[canonical not idempotent] {label}\n    first:  {canonical}\n    second: {canonical2}"
                    ));
                }
            }
            Ok(Err(error)) => failures.push(format!(
                "[canonical(parse(canonical)) errored] {label}\n    canonical: {canonical}\n    error: {error}"
            )),
            Err(_) => failures.push(format!(
                "[PANIC in canonical idempotence] {label}\n    canonical: {canonical}"
            )),
        }
    }
    assert!(ok_count > 0, "matrix produced no canonical Ok at all");
    assert!(
        failures.is_empty(),
        "{} canonical round-trip failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Conditions built directly as ASTs.
// ---------------------------------------------------------------------------

fn leaf(name: &str) -> MatchSpecCondition {
    MatchSpecCondition::MatchSpec(Box::new(base(name)))
}

fn leaf_v(name: &str, v: &str) -> MatchSpecCondition {
    let mut spec = base(name);
    spec.version = Some(version(v));
    MatchSpecCondition::MatchSpec(Box::new(spec))
}

fn and(a: MatchSpecCondition, b: MatchSpecCondition) -> MatchSpecCondition {
    MatchSpecCondition::And(Box::new(a), Box::new(b))
}

fn or(a: MatchSpecCondition, b: MatchSpecCondition) -> MatchSpecCondition {
    MatchSpecCondition::Or(Box::new(a), Box::new(b))
}

fn condition_matrix() -> Vec<(String, MatchSpecCondition)> {
    let mut cases: Vec<(String, MatchSpecCondition)> = Vec::new();
    let mut push = |label: &str, c: MatchSpecCondition| cases.push((label.to_string(), c));

    let a = || leaf("a");
    let b = || leaf("b");
    let c = || leaf("c");
    let d = || leaf("d");

    push("and(a,b)", and(a(), b()));
    push("or(a,b)", or(a(), b()));
    push("and(and(a,b),c)", and(and(a(), b()), c()));
    push("and(a,and(b,c))", and(a(), and(b(), c())));
    push("or(or(a,b),c)", or(or(a(), b()), c()));
    push("or(a,or(b,c))", or(a(), or(b(), c())));
    push("or(and(a,b),c)", or(and(a(), b()), c()));
    push("or(a,and(b,c))", or(a(), and(b(), c())));
    push("and(or(a,b),c)", and(or(a(), b()), c()));
    push("and(a,or(b,c))", and(a(), or(b(), c())));
    push("or(and(a,b),and(c,d))", or(and(a(), b()), and(c(), d())));
    push("and(or(a,b),or(c,d))", and(or(a(), b()), or(c(), d())));
    push("and(or(a,and(b,c)),d)", and(or(a(), and(b(), c())), d()));
    push("or(and(or(a,b),c),d)", or(and(or(a(), b()), c()), d()));

    // Deep chains.
    let mut left_and = a();
    let mut right_and = a();
    let mut left_mixed = a();
    for i in 0..10 {
        let name = format!("p{i}");
        left_and = and(left_and, leaf(&name));
        right_and = and(leaf(&name), right_and);
        left_mixed = if i % 2 == 0 {
            or(left_mixed, leaf(&name))
        } else {
            and(left_mixed, leaf(&name))
        };
    }
    push("deep-left-and", left_and);
    push("deep-right-and", right_and);
    push("deep-left-mixed", left_mixed);

    let mut right_or = leaf("z");
    for i in 0..10 {
        right_or = or(leaf(&format!("q{i}")), right_or);
    }
    push("deep-right-or", right_or);

    let mut right_mixed = leaf("z");
    for i in 0..10 {
        right_mixed = if i % 2 == 0 {
            and(leaf(&format!("r{i}")), right_mixed)
        } else {
            or(leaf(&format!("r{i}")), right_mixed)
        };
    }
    push("deep-right-mixed", right_mixed);

    // Ambiguous leaf names ("and"/"or" as substrings are legal names).
    push("ambiguous-pandoc", and(leaf("pandoc"), leaf("android")));
    push("ambiguous-orange", or(leaf("orange"), leaf("andy")));
    push(
        "ambiguous-sandwich-or",
        or(and(leaf("sand"), leaf("orc")), leaf("mandor")),
    );

    // Leaf names that ARE the keywords.
    push("keyword-and-leaf", leaf("and"));
    push("keyword-or-leaf", or(leaf("or"), leaf("b")));

    // Version renderings.
    push("compact-version", leaf_v("python", ">=3.6"));
    push("startswith-version", leaf_v("numpy", "1.2.*"));
    push("any-version", leaf_v("numpy", "*"));
    push("complex-version", leaf_v("python", ">=1,<2|==3"));
    push(
        "and-of-versions",
        and(leaf_v("python", ">=3.6"), leaf_v("numpy", "1.2.*")),
    );

    // Glob / regex names as leaves.
    push("glob-leaf", and(leaf("py*"), leaf("b")));
    push("regex-leaf", and(leaf("^py.*$"), leaf("b")));

    // Leaf with a quoted bracket value containing " and " / " or ".
    let mut tricky = base("python");
    tricky.license = Some("weird and license or value".to_string());
    push(
        "bracket-value-contains-keywords",
        and(
            MatchSpecCondition::MatchSpec(Box::new(tricky)),
            leaf("__linux"),
        ),
    );

    // Leaf with extras + flags.
    let mut rich = base("python");
    rich.extras = Some(vec!["docs".to_string()]);
    rich.flags = Some(vec!["cuda".parse().unwrap()]);
    push(
        "leaf-with-extras-flags",
        and(MatchSpecCondition::MatchSpec(Box::new(rich)), leaf("b")),
    );

    // Leaf with build (bracket form).
    let mut built = base("python");
    built.version = Some(version(">=3.8"));
    built.build = Some("py39*".parse().unwrap());
    push(
        "leaf-with-build",
        or(
            and(
                MatchSpecCondition::MatchSpec(Box::new(built)),
                leaf("__linux"),
            ),
            leaf("__win"),
        ),
    );

    cases
}

#[test]
fn condition_ast_canonical_roundtrip() {
    let options = strict_v3();
    let mut failures = Vec::new();
    let mut ok_count = 0;
    for (label, condition) in condition_matrix() {
        let mut spec = base("target");
        spec.condition = Some(condition.clone());

        let canonical = match catch_unwind(AssertUnwindSafe(|| spec.to_canonical_string())) {
            Ok(Ok(canonical)) => canonical,
            Ok(Err(_)) => continue,
            Err(_) => {
                failures.push(format!("[PANIC in canonical] {label}"));
                continue;
            }
        };
        ok_count += 1;
        match MatchSpec::from_str(&canonical, options) {
            Err(error) => failures.push(format!(
                "[condition canonical unparseable] {label}\n    canonical: {canonical}\n    error: {error}"
            )),
            Ok(reparsed) => {
                if reparsed.condition.as_ref() != Some(&condition) {
                    failures.push(format!(
                        "[condition canonical AST changed] {label}\n    canonical: {canonical}\n    original: {condition:?}\n    reparsed: {:?}",
                        reparsed.condition
                    ));
                }
            }
        }
    }
    assert!(ok_count > 0, "no condition rendered canonically at all");
    assert!(
        failures.is_empty(),
        "{} condition canonical failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Regression guard for legacy-Display scalar quoting.
///
/// The parser stores quoted scalar bracket values (`fn`, `license`, `subdir`,
/// `namespace`, `license_family`) with escape sequences in place; it only
/// unescapes `when=` and `flags=`. Legacy `write_scalar` must therefore emit
/// values verbatim inside a delimiter that keeps them intact, never escaped:
/// escaping a backslash-containing value made it reparse with doubled
/// backslashes, and escaping a quote-containing value made it silently
/// reparse to a different value.
#[test]
fn display_scalar_escaping_minimized() {
    let options = strict_v3();

    // Baseline fact about the (unchanged) parser: escapes are kept verbatim.
    let parsed = MatchSpec::from_str(r#"python[fn="a\b"]"#, options).unwrap();
    assert_eq!(
        parsed.file_name.as_deref(),
        Some(r"a\b"),
        "parser keeps scalar escapes verbatim"
    );

    // Backslash value: Display -> parse must give back the same file_name.
    let spec = MatchSpec {
        name: "python".parse().unwrap(),
        file_name: Some(r"a\b".to_string()),
        ..MatchSpec::default()
    };
    let rendered = spec.to_string();
    let reparsed = MatchSpec::from_str(&rendered, options)
        .unwrap_or_else(|e| panic!("Display output {rendered:?} unparseable: {e}"));
    assert_eq!(
        reparsed.file_name.as_deref(),
        Some(r"a\b"),
        "REGRESSION: Display rendered {rendered:?}; the backslash was doubled on reparse"
    );

    // Quote value: whenever the rendered form parses, the value must survive.
    let spec = MatchSpec {
        name: "python".parse().unwrap(),
        file_name: Some(r#"qu"ote"#.to_string()),
        ..MatchSpec::default()
    };
    let rendered = spec.to_string();
    if let Ok(reparsed) = MatchSpec::from_str(&rendered, options) {
        assert_eq!(
            reparsed.file_name.as_deref(),
            Some(r#"qu"ote"#),
            "REGRESSION: Display rendered {rendered:?}; the value silently changed on reparse"
        );
    }
}

/// Panic hunt on maximally weird specs. Any panic is a failure; round-trip
/// correctness is not asserted here.
#[test]
fn panic_hunt_on_weird_specs() {
    let mut cases: Vec<(String, MatchSpec)> = Vec::new();
    let mut push = |label: &str, spec: MatchSpec| cases.push((label.to_string(), spec));

    let mut spec = base("python");
    spec.extras = Some(vec![]);
    push("extras=Some(vec![])", spec);

    let mut spec = base("python");
    spec.flags = Some(vec![]);
    push("flags=Some(vec![])", spec);

    let mut spec = base("python");
    spec.track_features = Some(vec![]);
    push("track_features=Some(vec![])", spec);

    let mut spec = base("python");
    spec.extras = Some(vec![String::new()]);
    push("extras=[\"\"]", spec);

    let mut spec = base("python");
    spec.track_features = Some(vec![String::new()]);
    push("track_features=[\"\"]", spec);

    let mut spec = base("python");
    spec.track_features = Some(vec!["a,b".to_string(), "c d".to_string()]);
    push("track_features=[\"a,b\",\"c d\"]", spec);

    for field in [
        "file_name",
        "license",
        "license_family",
        "subdir",
        "namespace",
    ] {
        let mut spec = base("python");
        match field {
            "file_name" => spec.file_name = Some(String::new()),
            "license" => spec.license = Some(String::new()),
            "license_family" => spec.license_family = Some(String::new()),
            "subdir" => spec.subdir = Some(String::new()),
            _ => spec.namespace = Some(String::new()),
        }
        push(&format!("{field}=\"\""), spec);
    }

    let mut spec = base("python");
    spec.build = Some(StringMatcher::Exact(String::new()));
    push("build=Exact(\"\")", spec);

    let mut spec = base("python");
    spec.build = Some(StringMatcher::Exact("py 37_0".to_string()));
    push("build=Exact-with-space", spec);

    let mut spec = base("python");
    spec.build = Some(StringMatcher::Exact("py*".to_string()));
    push("build=Exact-containing-star", spec);

    let mut channel = (*mk_channel("https://repo.example/custom")).clone();
    channel.platforms = Some(vec![]);
    let mut spec = base("python");
    spec.channel = Some(Arc::new(channel));
    push("channel-platforms=Some(vec![])", spec);

    for url in [
        "https://user:p%40ss@repo.example/pkg.conda?a=1&b=2#frag",
        "file:///C:/some%20dir/pkg%20name.conda",
        "https://repo.example/pkg.conda#md5:0123456789abcdef0123456789abcdef",
    ] {
        let mut spec = base("python");
        spec.url = Some(Url::parse(url).unwrap());
        push(&format!("url={url}"), spec);
    }

    // Regex name with a space (constructible only via FromStr of a regex).
    let mut spec = base("^foo bar$");
    spec.version = Some(version(">=1"));
    push("regex-name-with-space", spec);

    // Glob name with bracket-ish characters.
    if let Ok(name) = "foo[ab]*".parse() {
        let spec = MatchSpec {
            name,
            ..MatchSpec::default()
        };
        push("glob-name-with-brackets", spec);
    }

    // Default (wildcard) name with everything unusual at once.
    let spec = MatchSpec {
        extras: Some(vec![]),
        flags: Some(vec![]),
        track_features: Some(vec![String::new()]),
        file_name: Some(String::new()),
        url: Some(Url::parse("https://u:p@h/x?q#f").unwrap()),
        ..MatchSpec::default()
    };
    push("wildcard-name-kitchen-sink", spec);

    let mut failures = Vec::new();
    for (label, spec) in cases {
        if catch_unwind(AssertUnwindSafe(|| spec.to_string())).is_err() {
            failures.push(format!("[PANIC in Display] {label}"));
        }
        if catch_unwind(AssertUnwindSafe(|| spec.to_canonical_string())).is_err() {
            failures.push(format!("[PANIC in to_canonical_string] {label}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} panics on weird specs:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
