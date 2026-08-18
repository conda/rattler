//! Property-based round-trip tests for [`MatchSpec`] formatting.
//!
//! Random specs and condition ASTs are rendered through `Display` and
//! `to_canonical_string` and parsed back. The central invariant of both
//! dialects: rendering may produce text the parser rejects (a loud failure),
//! but whenever the text parses it must describe the same query. Silent
//! divergence is always a bug. The canonical dialect is stricter: an `Ok`
//! must reparse equal (modulo documented URL credential redaction) and must
//! be idempotent.
//!
//! The fuzz property runs the opposite direction: fragment-composed raw
//! strings go straight into the parsers, which must never panic, and
//! anything they accept must obey the same loud-or-faithful rule.
//!
//! Documented equivalences the assertions allow:
//! * A reparsed [`Channel`] may carry a different display `name`. The URL
//!   and platform selector are the channel's identity, the name is derived.
//! * Canonical output redacts URL credentials, so URLs are compared after
//!   redaction.
//! * A `NamelessMatchSpec` with no fields at all renders as `*`, which
//!   reparses as `version: Any`, a matcher identical to `version: None`.

use std::sync::Arc;

use proptest::prelude::*;
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, MatchSpecCondition, NamelessMatchSpec, PackageName,
    PackageNameMatcher, ParseMatchSpecOptions, RepodataRevision, StringMatcher, VersionSpec,
};
use rattler_redaction::redact_credentials_from_url;
use url::Url;

fn strict_v3() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict()
        .with_repodata_revision(RepodataRevision::V3)
        .with_exact_names_only(false)
}

fn lenient_v3() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::lenient()
        .with_repodata_revision(RepodataRevision::V3)
        .with_exact_names_only(false)
}

fn channel_cfg() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::temp_dir())
}

// -------------------------------------------------------------------------
// Strategies
// -------------------------------------------------------------------------

/// Always-valid version text built from components; shrinks toward `0`.
fn version_text() -> BoxedStrategy<String> {
    (
        prop::collection::vec(0u8..30, 1..4),
        prop::option::weighted(0.15, 1u8..3),
    )
        .prop_map(|(segments, epoch)| {
            let segments = segments
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(".");
            match epoch {
                Some(epoch) => format!("{epoch}!{segments}"),
                None => segments,
            }
        })
        .boxed()
}

/// Always-valid package-name text; shrinks toward `a`.
fn package_name_text() -> BoxedStrategy<String> {
    prop::collection::vec(
        prop_oneof![
            8 => prop::char::range('a', 'z'),
            1 => prop::char::range('0', '9'),
            1 => prop::sample::select(&['_', '-', '.'][..]),
        ],
        1..8,
    )
    .prop_map(|mut chars| {
        // Keep the first character a letter so every value is a valid name.
        if !chars[0].is_ascii_lowercase() {
            chars[0] = 'a';
        }
        chars.into_iter().collect()
    })
    .boxed()
}

fn name_matcher() -> BoxedStrategy<PackageNameMatcher> {
    prop_oneof![
        // Constructive arms first: always valid, so shrinking moves through
        // them without hitting parse-rejection walls.
        4 => package_name_text().prop_map(|name| {
            PackageNameMatcher::Exact(
                PackageName::try_from(name).expect("constructed package name must be valid"),
            )
        }),
        // Names that read like condition keywords.
        1 => prop::sample::select(&["and", "or", "pandoc", "android"][..]).prop_map(|name| {
            PackageNameMatcher::Exact(PackageName::try_from(name.to_string()).expect("valid name"))
        }),
        // Globs and regexes, including regex bodies with version-constraint
        // characters and whitespace.
        1 => prop_oneof![
            package_name_text().prop_map(|name| format!("{name}*")),
            package_name_text().prop_map(|name| format!("*{name}")),
            Just("^py(?!py).*$".to_string()),
            Just("^foo bar$".to_string()),
            Just("^py.*$".to_string()),
        ]
        .prop_filter_map("valid name matcher", |name| {
            name.parse::<PackageNameMatcher>().ok()
        }),
    ]
    .boxed()
}

fn version_spec() -> BoxedStrategy<VersionSpec> {
    let operator = prop::sample::select(&[">=", ">", "<=", "<", "==", "!="][..]);
    prop_oneof![
        // Constructive arms first, for the same shrinking reason.
        4 => (operator, version_text()).prop_map(|(op, version)| format!("{op}{version}")),
        1 => Just("*".to_string()),
        1 => version_text().prop_map(|version| format!("{version}.*")),
        1 => (version_text(), version_text()).prop_map(|(low, high)| format!(">={low},<{high}")),
        1 => (version_text(), version_text()).prop_map(|(a, b)| format!(">={a}|<{b}")),
        // Oddities the constructive arms do not produce.
        1 => prop_oneof![
            Just(">=1.2.3dev_".to_string()),
            Just("==1!2.0".to_string()),
            Just("~=1.2".to_string()),
        ],
    ]
    .prop_map(|spec| {
        VersionSpec::from_str(&spec, rattler_conda_types::ParseStrictness::Lenient)
            .expect("constructed version spec must parse")
    })
    .boxed()
}

/// Exact-matcher text; shrinks toward `a`.
fn matcher_text() -> BoxedStrategy<String> {
    prop::collection::vec(
        prop_oneof![
            6 => prop::char::range('a', 'z'),
            1 => prop::char::range('0', '9'),
            1 => Just('_'),
        ],
        1..8,
    )
    .prop_map(|chars| chars.into_iter().collect())
    .boxed()
}

/// String matchers as the parser produces them: their rendered text always
/// reparses to the identical matcher. Constructive arms first so failures
/// shrink toward a plain exact matcher.
fn parsed_string_matcher() -> BoxedStrategy<StringMatcher> {
    prop_oneof![
        4 => matcher_text(),
        1 => Just("*".to_string()),
        1 => (matcher_text(), matcher_text()).prop_map(|(a, b)| format!("{a}*{b}")),
        1 => matcher_text().prop_map(|body| format!("^{body}.*$")),
    ]
    .prop_map(|value| {
        value
            .parse::<StringMatcher>()
            .expect("constructed matcher must parse")
    })
    .boxed()
}

fn string_matcher() -> BoxedStrategy<StringMatcher> {
    prop_oneof![
        6 => parsed_string_matcher(),
        // Regex matchers with arbitrary bodies stay behind a filter because
        // not every body compiles; listed late so shrinking prefers the
        // constructive arms.
        1 => "\\^py.*\\$".prop_filter_map("valid matcher", |value| {
            value.parse::<StringMatcher>().ok()
        }),
        // Programmatic construction can force states whose text reparses as a
        // different matcher variant, e.g. an Exact matcher that reads as a
        // glob; see `matcher_equivalent`.
        1 => Just(StringMatcher::Exact("cuda*".to_string())),
    ]
    .boxed()
}

/// Fragments the fuzz property concatenates into parser input. Grammar
/// pieces dominate so a decent share of inputs parses; the rest exercises
/// rejection paths. Ordered inert-first because `select` shrinks toward the
/// front of the slice, which keeps minimal repros readable.
const FUZZ_FRAGMENTS: &[&str] = &[
    "python",
    "conda-forge",
    "linux-64",
    "noarch",
    " ",
    "1.2.*",
    ">=1.2,<2",
    "==1!2.0dev_",
    "^py.*$",
    "0123456789abcdef0123456789abcdef",
    "version=",
    "build=",
    "fn=",
    "channel=",
    "subdir=",
    "md5=",
    "when=",
    "extras=[docs,tests]",
    "flags=[cuda]",
    "::",
    ":",
    "/",
    "[",
    "]",
    "(",
    ")",
    "*",
    "=",
    ",",
    "\"",
    "'",
    "\\",
    "#",
    ";",
    "$",
    " if ",
    " and ",
    " or ",
    "https://",
    "[::1]",
    "user:secret@repo.example",
    "?token",
    "\u{1F980}",
];

fn fuzz_input() -> BoxedStrategy<String> {
    let fragment = prop_oneof![
        6 => prop::sample::select(FUZZ_FRAGMENTS).prop_map(str::to_string),
        1 => "[ -~]{1,6}",
    ];
    prop::collection::vec(fragment, 0..8)
        .prop_map(|fragments| fragments.concat())
        .boxed()
}

/// Scalar values mixing plain text with the characters that stress quoting:
/// quotes, backslashes, brackets, `#`, commas, spaces, and raw unicode.
/// Built from a char vec so shrinking removes characters and drifts the rest
/// toward `a`.
fn scalar_value() -> BoxedStrategy<String> {
    prop::collection::vec(
        prop_oneof![
            6 => prop::char::range('a', 'z'),
            2 => prop::sample::select(
                &[' ', '"', '\'', '\\', '[', ']', '#', ',', '=', ':', '?'][..]
            ),
            1 => prop::char::any(),
        ],
        1..12,
    )
    .prop_map(|chars| chars.into_iter().collect())
    .boxed()
}

fn extras() -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(
        prop_oneof![
            4 => "[a-z][a-z0-9_.+-]{0,8}",
            // Invalid group names must produce loud failures or errors.
            1 => prop_oneof![Just("Docs".to_string()), Just("a,b".to_string()), Just(String::new())],
        ],
        1..3,
    ).boxed()
}

fn flags() -> BoxedStrategy<Vec<StringMatcher>> {
    prop::collection::vec(
        prop_oneof![
            "[a-z][a-z0-9_]{0,6}".prop_map(StringMatcher::Exact),
            Just(StringMatcher::Exact("cuda*".to_string())),
        ],
        1..3,
    )
    .boxed()
}

fn track_features() -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(
        prop_oneof![
            4 => "[a-z][a-z0-9_]{0,6}",
            1 => prop_oneof![Just("a b".to_string()), Just(String::new())],
        ],
        1..3,
    )
    .boxed()
}

/// Channels composed from parts instead of a hand-picked list, so the
/// randomness reaches the dimensions bugs have lived in: names whose last
/// segment is a platform, multi-segment label paths, IPv6 hosts, embedded
/// credentials, and platform selectors on any of them.
fn channel() -> BoxedStrategy<Arc<Channel>> {
    let segment = prop_oneof![
        3 => "[a-z][a-z0-9-]{0,8}",
        1 => Just("linux-64".to_string()),
        1 => Just("noarch".to_string()),
    ];
    let name = prop_oneof![
        4 => prop::collection::vec(segment, 1..4).prop_map(|segments| segments.join("/")),
        1 => Just("*".to_string()),
    ];
    let host = prop_oneof![
        3 => Just("repo.example".to_string()),
        1 => Just("[::1]".to_string()),
        1 => Just("user:secret@repo.example".to_string()),
    ];
    let selector = prop::option::weighted(
        0.3,
        prop_oneof![
            Just("[linux-64]".to_string()),
            Just("[linux-64,noarch]".to_string()),
        ],
    );

    prop_oneof![
        (name.clone(), selector.clone())
            .prop_map(|(name, selector)| format!("{name}{}", selector.unwrap_or_default())),
        (host, name, selector).prop_map(|(host, name, selector)| {
            format!("https://{host}/{name}{}", selector.unwrap_or_default())
        }),
    ]
    .prop_map(|channel| {
        // Every composed channel is valid by construction, keeping the shrink
        // chain free of parse-rejection walls.
        Arc::new(Channel::from_str(&channel, &channel_cfg()).expect("constructed channel"))
    })
    .boxed()
}

fn url() -> BoxedStrategy<Url> {
    prop_oneof![
        "[a-z0-9-]{1,8}".prop_map(|segment| {
            Url::parse(&format!("https://repo.example/{segment}.conda")).unwrap()
        }),
        Just(Url::parse("https://u:p@repo.example/pkg.conda?auth=tok#frag").unwrap()),
        Just(Url::parse("https://repo.example/pkg.conda#sha256:deadbeef").unwrap()),
    ]
    .boxed()
}

prop_compose! {
    /// A random match spec without a condition (conditions are layered on
    /// separately so leaves never nest).
    fn bare_spec()(
        name in name_matcher(),
        version in prop::option::weighted(0.5, version_spec()),
        build in prop::option::weighted(0.3, string_matcher()),
        build_number in prop::option::weighted(0.15, Just(">=2".parse().unwrap())),
        file_name in prop::option::weighted(0.2, scalar_value()),
        extras in prop::option::weighted(0.2, extras()),
        flags in prop::option::weighted(0.2, flags()),
        channel in prop::option::weighted(0.3, channel()),
        subdir in prop::option::weighted(0.2, prop_oneof![
            Just("linux-64".to_string()),
            Just("noarch".to_string()),
            Just("plain".to_string()),
        ]),
        namespace in prop::option::weighted(0.1, prop_oneof![
            Just("ns".to_string()),
            Just("name#space".to_string()),
        ]),
        md5 in prop::option::weighted(0.1, any::<[u8; 16]>()),
        sha256 in prop::option::weighted(0.1, any::<[u8; 32]>()),
        url in prop::option::weighted(0.15, url()),
        license in prop::option::weighted(0.2, scalar_value()),
        license_family in prop::option::weighted(0.1, "[A-Z]{2,6}"),
        track_features in prop::option::weighted(0.15, track_features()),
    ) -> MatchSpec {
        MatchSpec {
            name,
            version,
            build,
            build_number,
            file_name,
            extras,
            flags,
            channel,
            subdir,
            namespace,
            md5: md5.map(Into::into),
            sha256: sha256.map(Into::into),
            url,
            license,
            license_family,
            condition: None,
            track_features,
        }
    }
}

fn condition() -> BoxedStrategy<MatchSpecCondition> {
    let leaf = (
        name_matcher(),
        prop::option::weighted(0.6, version_spec()),
        prop::option::weighted(0.2, parsed_string_matcher()),
        // The grammar cannot represent a nested `when` on a leaf: canonical
        // must return NestedWhen and legacy Display must fail loudly, never
        // panic or drop the condition.
        prop::option::weighted(0.08, "[a-z]{1,6}"),
    )
        .prop_map(|(name, version, build, nested)| {
            MatchSpecCondition::MatchSpec(Box::new(MatchSpec {
                name,
                version,
                build,
                condition: nested.and_then(|nested| {
                    let name = nested.parse().ok()?;
                    Some(MatchSpecCondition::MatchSpec(Box::new(MatchSpec {
                        name,
                        ..MatchSpec::default()
                    })))
                }),
                ..MatchSpec::default()
            }))
        });
    leaf.prop_recursive(4, 24, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| MatchSpecCondition::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner)
                .prop_map(|(a, b)| MatchSpecCondition::Or(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

fn spec_with_optional_condition() -> BoxedStrategy<MatchSpec> {
    (bare_spec(), prop::option::weighted(0.4, condition()))
        .prop_map(|(mut spec, condition)| {
            spec.condition = condition;
            spec
        })
        .boxed()
}

// -------------------------------------------------------------------------
// Equivalence
// -------------------------------------------------------------------------

/// Compares a reparsed channel against the original: the base URL (optionally
/// after credential redaction) and platform selector are the identity; the
/// display name is derived and may differ.
fn channel_equivalent(original: &Channel, reparsed: &Channel, redacted: bool) -> bool {
    let original_url = if redacted {
        redact_credentials_from_url(original.base_url.url())
    } else {
        (**original.base_url.url()).clone()
    };
    **reparsed.base_url.url() == original_url && reparsed.platforms == original.platforms
}

/// A reparsed matcher is faithful when it is identical, or when it equals
/// what the matcher's own rendered text parses to. The latter covers
/// programmatically constructed matchers whose text belongs to a different
/// variant, e.g. `Exact("cuda*")` widens to the glob `cuda*`. The grammar
/// cannot distinguish those states, so their text identity is the best any
/// renderer can preserve.
fn matcher_equivalent(original: &StringMatcher, reparsed: &StringMatcher) -> bool {
    original == reparsed
        || original
            .to_string()
            .parse::<StringMatcher>()
            .is_ok_and(|normalized| normalized == *reparsed)
}

fn option_matcher_equivalent(
    original: Option<&StringMatcher>,
    reparsed: Option<&StringMatcher>,
) -> bool {
    match (original, reparsed) {
        (None, None) => true,
        (Some(original), Some(reparsed)) => matcher_equivalent(original, reparsed),
        _ => false,
    }
}

/// Track features have no quoting grammar: the rendered value re-splits on
/// spaces and commas, so elements containing those separators (or empty
/// elements) are compared against the re-split form.
fn track_features_equivalent(original: Option<&[String]>, reparsed: Option<&[String]>) -> bool {
    match (original, reparsed) {
        (None, None) => true,
        (Some(original), Some(reparsed)) => {
            let resplit: Vec<&str> = original
                .iter()
                .flat_map(|feature| feature.split([',', ' ']))
                .filter(|feature| !feature.is_empty())
                .collect();
            reparsed.iter().map(String::as_str).eq(resplit)
        }
        _ => false,
    }
}

/// Asserts that `reparsed` describes the same query as `original`, allowing
/// only the documented equivalences. `redacted` selects canonical semantics.
fn assert_faithful(original: &MatchSpec, reparsed: &MatchSpec, rendered: &str, redacted: bool) {
    let expected_url = if redacted {
        original.url.as_ref().map(redact_credentials_from_url)
    } else {
        original.url.clone()
    };
    let channel_ok = match (original.channel.as_deref(), reparsed.channel.as_deref()) {
        (None, None) => true,
        (Some(original), Some(reparsed)) => channel_equivalent(original, reparsed, redacted),
        _ => false,
    };

    let mut divergences = Vec::new();
    let mut check = |field: &'static str, equal: bool| {
        if !equal {
            divergences.push(field);
        }
    };
    check("name", reparsed.name == original.name);
    check("version", reparsed.version == original.version);
    check(
        "build",
        option_matcher_equivalent(original.build.as_ref(), reparsed.build.as_ref()),
    );
    check(
        "build_number",
        reparsed.build_number == original.build_number,
    );
    check("file_name", reparsed.file_name == original.file_name);
    check("extras", reparsed.extras == original.extras);
    check(
        "flags",
        match (original.flags.as_ref(), reparsed.flags.as_ref()) {
            (None, None) => true,
            (Some(original), Some(reparsed)) => {
                original.len() == reparsed.len()
                    && original
                        .iter()
                        .zip(reparsed)
                        .all(|(original, reparsed)| matcher_equivalent(original, reparsed))
            }
            _ => false,
        },
    );
    check("channel", channel_ok);
    check("subdir", reparsed.subdir == original.subdir);
    check("namespace", reparsed.namespace == original.namespace);
    check("md5", reparsed.md5 == original.md5);
    check("sha256", reparsed.sha256 == original.sha256);
    check("url", reparsed.url == expected_url);
    check("license", reparsed.license == original.license);
    check(
        "license_family",
        reparsed.license_family == original.license_family,
    );
    check("condition", reparsed.condition == original.condition);
    check(
        "track_features",
        track_features_equivalent(
            original.track_features.as_deref(),
            reparsed.track_features.as_deref(),
        ),
    );

    assert!(
        divergences.is_empty(),
        "rendered {rendered:?} reparsed with silently diverging fields {divergences:?}:\n  original: {original:?}\n  reparsed: {reparsed:?}"
    );
}

// -------------------------------------------------------------------------
// Properties
// -------------------------------------------------------------------------

proptest! {
    // A full spec plus a condition tree is a big value; give the shrinker
    // enough budget to minimize it completely.
    #![proptest_config(ProptestConfig {
        max_shrink_iters: 8192,
        ..ProptestConfig::default()
    })]

    /// Legacy `Display` never lies: its output either fails to parse (loud)
    /// or reparses to the same query.
    #[test]
    fn display_never_silently_diverges(spec in spec_with_optional_condition()) {
        let rendered = spec.to_string();
        if let Ok(reparsed) = MatchSpec::from_str(&rendered, strict_v3()) {
            assert_faithful(&spec, &reparsed, &rendered, false);
        }
    }

    /// Every canonical `Ok` reparses to an equal spec (modulo credential
    /// redaction) and the canonical form is idempotent. Errors are allowed;
    /// panics and silent divergence are not.
    #[test]
    fn canonical_is_verified_and_idempotent(spec in spec_with_optional_condition()) {
        if let Ok(canonical) = spec.to_canonical_string() {
            let reparsed = MatchSpec::from_str(&canonical, strict_v3()).unwrap_or_else(|error| {
                panic!("canonical {canonical:?} does not reparse: {error}")
            });
            assert_faithful(&spec, &reparsed, &canonical, true);
            let again = reparsed.to_canonical_string().unwrap_or_else(|error| {
                panic!("reparsed canonical {canonical:?} fails to re-render: {error}")
            });
            prop_assert_eq!(&again, &canonical, "canonical form is not idempotent");
        }
    }

    /// Condition ASTs survive both dialects: whenever the rendered spec
    /// parses, the reparsed condition is the identical tree.
    #[test]
    fn condition_ast_roundtrips(condition in condition()) {
        let spec = MatchSpec {
            name: "target".parse().unwrap(),
            condition: Some(condition),
            ..MatchSpec::default()
        };

        let rendered = spec.to_string();
        if let Ok(reparsed) = MatchSpec::from_str(&rendered, strict_v3()) {
            prop_assert_eq!(
                &reparsed.condition, &spec.condition,
                "Display {} reparsed to a different condition tree", rendered
            );
        }

        if let Ok(canonical) = spec.to_canonical_string() {
            let reparsed = MatchSpec::from_str(&canonical, strict_v3()).unwrap_or_else(|error| {
                panic!("canonical {canonical:?} does not reparse: {error}")
            });
            prop_assert_eq!(
                &reparsed.condition, &spec.condition,
                "canonical {} reparsed to a different condition tree", canonical
            );
        }
    }

    /// `NamelessMatchSpec` follows the same never-lie rule as the named form.
    #[test]
    fn nameless_display_never_silently_diverges(spec in spec_with_optional_condition()) {
        let nameless = NamelessMatchSpec::from(spec);
        let rendered = nameless.to_string();
        let Ok(reparsed) = NamelessMatchSpec::from_str(&rendered, strict_v3()) else {
            return Ok(());
        };

        // The one documented exception: a spec with no fields at all renders
        // as `*`, which reparses as `version: Any` and matches the same set
        // as `version: None`.
        if nameless == NamelessMatchSpec::default() {
            prop_assert_eq!(rendered.as_str(), "*");
            return Ok(());
        }

        let original = MatchSpec::from_nameless(nameless, "x".parse().unwrap());
        let reparsed = MatchSpec::from_nameless(reparsed, "x".parse().unwrap());
        assert_faithful(&original, &reparsed, &rendered, false);
    }

    /// The parsers never panic on arbitrary input, and any spec they accept
    /// follows the loud-or-faithful rule when rendered again.
    #[test]
    fn parser_never_panics_and_accepted_input_roundtrips(input in fuzz_input()) {
        for options in [strict_v3(), lenient_v3()] {
            if let Ok(spec) = MatchSpec::from_str(&input, options) {
                let rendered = spec.to_string();
                if let Ok(reparsed) = MatchSpec::from_str(&rendered, strict_v3()) {
                    assert_faithful(&spec, &reparsed, &rendered, false);
                }
                // Must not panic; Ok is verified internally by round-trip.
                let _ = spec.to_canonical_string();
            }
            let _ = NamelessMatchSpec::from_str(&input, options);
        }
        let _ = Channel::from_str(&input, &channel_cfg());
    }
}
