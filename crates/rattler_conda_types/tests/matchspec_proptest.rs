//! Property-based round-trip tests for [`MatchSpec`] formatting.
//!
//! Random specs and condition ASTs are rendered through `Display` and
//! `to_canonical_string` and parsed back. The central invariant of both
//! dialects: rendering may produce text the parser rejects (a loud failure),
//! but whenever the text parses it must describe the same query. Silent
//! divergence is always a bug. The canonical dialect is stricter: an `Ok`
//! must reparse equal (modulo the documented URL stripping) and must be
//! idempotent.
//!
//! The fuzz property runs the opposite direction: fragment-composed raw
//! strings go straight into the parsers, which must never panic, and
//! anything they accept must obey the same loud-or-faithful rule.
//!
//! Documented equivalences the assertions allow:
//! * A reparsed [`Channel`] may carry a different display `name`. The URL
//!   and platform selector are the channel's identity, the name is derived.
//! * Canonical output strips URL credentials, so URLs are compared after
//!   stripping.
//! * A `NamelessMatchSpec` with no fields at all renders as `*`, which
//!   reparses as `version: Any`, a matcher identical to `version: None`.

use proptest::prelude::*;
use rattler_conda_types::proptest::{match_spec, match_spec_condition};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, MatchSpecCondition, NamelessMatchSpec,
    ParseMatchSpecOptions, RepodataRevision, StringMatcher,
};
use rattler_redaction::strip_url_for_serialization;

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
//
// The well-formed value generators live in `rattler_conda_types::proptest`
// (behind the `proptest` feature) so other crates can reuse them. This file
// only adds the adversarial layer: constructible-but-unrepresentable states
// the never-lie properties must also cover, and the raw-input fuzz.

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

/// Constructible states the library generators deliberately exclude because
/// they violate a field grammar or matcher text identity. The never-lie
/// properties must hold for them too: loud failures or documented
/// equivalences, never silent divergence.
#[derive(Debug, Clone)]
enum Mutation {
    /// An Exact matcher whose text reads as a glob; see `matcher_equivalent`.
    GlobLikeExactBuild,
    GlobLikeExactFlag,
    /// A regex build with an arbitrary body.
    WildRegexBuild(StringMatcher),
    /// An extras element that violates the CEP 44 group-name grammar.
    InvalidExtra(String),
    /// A track feature containing a list delimiter, or nothing at all.
    InvalidTrackFeature(String),
    /// A condition leaf carrying its own `when`, which the grammar cannot
    /// represent.
    NestedWhen,
}

fn mutation() -> BoxedStrategy<Mutation> {
    prop_oneof![
        Just(Mutation::GlobLikeExactBuild),
        Just(Mutation::GlobLikeExactFlag),
        "\\^py.*\\$"
            .prop_filter_map("valid matcher", |value| value.parse::<StringMatcher>().ok())
            .prop_map(Mutation::WildRegexBuild),
        prop_oneof![
            Just("Docs".to_string()),
            Just("a,b".to_string()),
            Just(String::new()),
        ]
        .prop_map(Mutation::InvalidExtra),
        prop_oneof![Just("a b".to_string()), Just(String::new())]
            .prop_map(Mutation::InvalidTrackFeature),
        Just(Mutation::NestedWhen),
    ]
    .boxed()
}

/// The library's well-formed specs, with an occasional adversarial mutation
/// layered on top.
fn spec_under_test() -> BoxedStrategy<MatchSpec> {
    (match_spec(), prop::option::weighted(0.3, mutation()))
        .prop_map(|(mut spec, mutation)| {
            match mutation {
                None => {}
                Some(Mutation::GlobLikeExactBuild) => {
                    spec.build = Some(StringMatcher::Exact("cuda*".to_string()));
                }
                Some(Mutation::GlobLikeExactFlag) => {
                    spec.flags
                        .get_or_insert_with(Vec::new)
                        .push(StringMatcher::Exact("cuda*".to_string()));
                }
                Some(Mutation::WildRegexBuild(matcher)) => {
                    spec.build = Some(matcher);
                }
                Some(Mutation::InvalidExtra(extra)) => {
                    spec.extras.get_or_insert_with(Vec::new).push(extra);
                }
                Some(Mutation::InvalidTrackFeature(feature)) => {
                    spec.track_features
                        .get_or_insert_with(Vec::new)
                        .push(feature);
                }
                Some(Mutation::NestedWhen) => {
                    let leaf = MatchSpec {
                        condition: Some(MatchSpecCondition::MatchSpec(Box::new(
                            MatchSpec::from_str("__linux", strict_v3()).unwrap(),
                        ))),
                        ..MatchSpec::from_str("python", strict_v3()).unwrap()
                    };
                    spec.condition = Some(MatchSpecCondition::MatchSpec(Box::new(leaf)));
                }
            }
            spec
        })
        .boxed()
}

// -------------------------------------------------------------------------
// Equivalence
// -------------------------------------------------------------------------

/// Compares a reparsed channel against the original: the base URL (optionally
/// after credentials are stripped) and platform selector are the identity; the
/// display name is derived and may differ.
fn channel_equivalent(original: &Channel, reparsed: &Channel, stripped: bool) -> bool {
    let original_url = if stripped {
        strip_url_for_serialization(original.base_url.url())
    } else {
        (**original.base_url.url()).clone()
    };
    // A channel URL is a directory either way it is written.
    reparsed.base_url.url().as_str().trim_end_matches('/')
        == original_url.as_str().trim_end_matches('/')
        && reparsed.platforms == original.platforms
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

/// Track features have no quoting grammar in either form: the rendered value
/// re-splits on spaces and commas, so elements containing those separators (or
/// empty elements) are compared against the re-split form.
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
/// only the documented equivalences. `stripped` selects canonical semantics.
fn assert_faithful(original: &MatchSpec, reparsed: &MatchSpec, rendered: &str, stripped: bool) {
    let expected_url = if stripped {
        original.url.as_ref().map(strip_url_for_serialization)
    } else {
        original.url.clone()
    };
    let channel_ok = match (original.channel.as_deref(), reparsed.channel.as_deref()) {
        (None, None) => true,
        (Some(original), Some(reparsed)) => channel_equivalent(original, reparsed, stripped),
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
    fn display_never_silently_diverges(spec in spec_under_test()) {
        let rendered = spec.to_string();
        if let Ok(reparsed) = MatchSpec::from_str(&rendered, strict_v3()) {
            assert_faithful(&spec, &reparsed, &rendered, false);
        }
    }

    /// Every canonical `Ok` reparses to an equal spec (modulo stripped
    /// credentials) and the canonical form is idempotent. Errors are allowed;
    /// panics and silent divergence are not.
    #[test]
    fn canonical_is_verified_and_idempotent(spec in spec_under_test()) {
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
    fn condition_ast_roundtrips(condition in match_spec_condition(4)) {
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
    fn nameless_display_never_silently_diverges(spec in spec_under_test()) {
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
                // Must not panic; fidelity of Ok results is asserted by the
                // canonical property.
                let _ = spec.to_canonical_string();
            }
            let _ = NamelessMatchSpec::from_str(&input, options);
        }
        let _ = Channel::from_str(&input, &channel_cfg());
    }
}
