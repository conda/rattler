//! Property-testing strategies for the types in this crate.
//!
//! Available behind the implicit feature created by the optional `proptest`
//! dependency. Every strategy produces values that are valid by construction,
//! so shrinking never stalls on parse-rejection walls. Strategies for types
//! owned by this crate are also wired up as [`Arbitrary`] implementations, so
//! `any::<MatchSpec>()` works out of the box and composes
//! (`any::<Vec<MatchSpec>>()`). [`VersionSpec`] is owned by
//! `rattler_conda_version`, so use [`version_spec`] directly.
//!
//! The generated values are well-formed: field values follow their grammars
//! (extras and flags are valid group names, matchers are parser-reachable)
//! and specs round-trip through the crate's parsers. Tests that need
//! deliberately broken states should mutate the generated values themselves.
//!
//! The exact distributions are test helpers, not API: they may change in
//! minor releases as coverage improves.

use std::sync::Arc;

use ::proptest::arbitrary::Arbitrary;
use ::proptest::prelude::*;

use crate::{
    Channel, ChannelConfig, MatchSpec, MatchSpecCondition, NamelessMatchSpec, PackageName,
    PackageNameMatcher, ParseStrictness, StringMatcher, VersionSpec,
};

/// Creates the channel configuration used to parse generated channel strings.
fn channel_cfg() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(std::env::temp_dir())
}

/// Generates valid version text for [`version_spec`].
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

/// Generates valid package-name text for [`package_name`].
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

/// Generates package names, including names that resemble condition keywords.
pub fn package_name() -> BoxedStrategy<PackageName> {
    prop_oneof![
        4 => package_name_text(),
        1 => prop::sample::select(&["and", "or", "pandoc", "android"][..]).prop_map(str::to_string),
    ]
    .prop_map(|name| PackageName::try_from(name).expect("constructed package name must be valid"))
    .boxed()
}

/// Generates parser-reachable exact, glob, and regex package-name matchers.
pub fn package_name_matcher() -> BoxedStrategy<PackageNameMatcher> {
    prop_oneof![
        // Constructive arms first: always valid, so shrinking moves through
        // them without hitting parse-rejection walls.
        5 => package_name().prop_map(PackageNameMatcher::Exact),
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

/// Generates valid version specs across common constraint forms and parser
/// edge cases.
pub fn version_spec() -> BoxedStrategy<VersionSpec> {
    let operator = prop::sample::select(&[">=", ">", "<=", "<", "==", "!="][..]);
    prop_oneof![
        // Constructive arms first, so shrinking does not hit parse-rejection
        // walls.
        4 => (operator, version_text()).prop_map(|(op, version)| format!("{op}{version}")),
        1 => Just("*".to_string()),
        1 => version_text().prop_map(|version| format!("{version}.*")),
        1 => (version_text(), version_text()).prop_map(|(low, high)| format!(">={low},<{high}")),
        1 => (version_text(), version_text()).prop_map(|(a, b)| format!(">={a}|<{b}")),
        // Cover parser oddities that the constructive arms do not produce.
        1 => prop_oneof![
            Just(">=1.2.3dev_".to_string()),
            Just("==1!2.0".to_string()),
            Just("~=1.2".to_string()),
        ],
    ]
    .prop_map(|spec| {
        VersionSpec::from_str(&spec, ParseStrictness::Lenient)
            .expect("constructed version spec must parse")
    })
    .boxed()
}

/// Generates plain matcher text for [`string_matcher`].
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

/// Generates string matchers whose rendered form reparses to the same value.
pub fn string_matcher() -> BoxedStrategy<StringMatcher> {
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

/// Generates scalar field values that exercise quoting and escaping.
fn bracket_scalar() -> BoxedStrategy<String> {
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

/// Generates valid CEP 44 extra names for match specs.
fn extras() -> BoxedStrategy<Vec<String>> {
    prop::collection::vec("[a-z][a-z0-9_.+-]{0,8}", 1..3).boxed()
}

/// Generates valid variant flag matchers for match specs.
fn flags() -> BoxedStrategy<Vec<StringMatcher>> {
    prop::collection::vec("[a-z][a-z0-9_]{0,6}".prop_map(StringMatcher::Exact), 1..3).boxed()
}

/// Generates valid track-feature names for match specs.
fn track_features() -> BoxedStrategy<Vec<String>> {
    prop::collection::vec("[a-z][a-z0-9_]{0,6}", 1..3).boxed()
}

/// Generates channels that cover named channels, URLs, credentials, IPv6, and
/// platform selectors.
pub fn channel() -> BoxedStrategy<Channel> {
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
        Channel::from_str(&channel, &channel_cfg()).expect("constructed channel")
    })
    .boxed()
}

/// Generates package URLs that exercise credentials, queries, and fragments.
fn spec_url() -> BoxedStrategy<url::Url> {
    prop_oneof![
        "[a-z0-9-]{1,8}".prop_map(|segment| {
            url::Url::parse(&format!("https://repo.example/{segment}.conda")).unwrap()
        }),
        Just(url::Url::parse("https://u:p@repo.example/pkg.conda?auth=tok#frag").unwrap()),
        Just(url::Url::parse("https://repo.example/pkg.conda#sha256:deadbeef").unwrap()),
    ]
    .boxed()
}

/// Generates match-spec condition trees up to the requested recursion depth.
pub fn match_spec_condition(depth: u32) -> BoxedStrategy<MatchSpecCondition> {
    let leaf = (
        package_name_matcher(),
        prop::option::weighted(0.6, version_spec()),
        prop::option::weighted(0.2, string_matcher()),
    )
        .prop_map(|(name, version, build)| {
            MatchSpecCondition::MatchSpec(Box::new(MatchSpec {
                name,
                version,
                build,
                ..MatchSpec::default()
            }))
        });
    leaf.prop_recursive(depth, 24, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| MatchSpecCondition::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner)
                .prop_map(|(a, b)| MatchSpecCondition::Or(Box::new(a), Box::new(b))),
        ]
    })
    .boxed()
}

/// Generates well-formed match specs that exercise every field.
///
/// This constructs [`MatchSpec`] exhaustively so a new field causes a compile
/// error until the strategy defines how to generate it.
pub fn match_spec() -> BoxedStrategy<MatchSpec> {
    let fields = (
        (
            package_name_matcher(),
            prop::option::weighted(0.5, version_spec()),
            prop::option::weighted(0.3, string_matcher()),
            prop::option::weighted(0.15, Just(">=2".parse().unwrap())),
            prop::option::weighted(0.2, bracket_scalar()),
            prop::option::weighted(0.2, extras()),
            prop::option::weighted(0.2, flags()),
        ),
        (
            prop::option::weighted(0.3, channel()),
            prop::option::weighted(
                0.2,
                prop_oneof![
                    Just("linux-64".to_string()),
                    Just("noarch".to_string()),
                    Just("plain".to_string()),
                ],
            ),
            prop::option::weighted(
                0.1,
                prop_oneof![Just("ns".to_string()), Just("name#space".to_string()),],
            ),
            prop::option::weighted(0.1, any::<[u8; 16]>()),
            prop::option::weighted(0.1, any::<[u8; 32]>()),
            prop::option::weighted(0.15, spec_url()),
        ),
        (
            prop::option::weighted(0.2, bracket_scalar()),
            prop::option::weighted(0.1, "[A-Z]{2,6}"),
            prop::option::weighted(0.2, match_spec_condition(4)),
            prop::option::weighted(0.15, track_features()),
        ),
    );

    fields
        .prop_map(
            |(
                (name, version, build, build_number, file_name, extras, flags),
                (channel, subdir, namespace, md5, sha256, url),
                (license, license_family, condition, track_features),
            )| MatchSpec {
                name,
                version,
                build,
                build_number,
                file_name,
                extras,
                flags,
                channel: channel.map(Arc::new),
                subdir,
                namespace,
                md5: md5.map(Into::into),
                sha256: sha256.map(Into::into),
                url,
                license,
                license_family,
                condition,
                track_features,
            },
        )
        .boxed()
}

/// Generates nameless match specs from the complete [`match_spec`] strategy.
pub fn nameless_match_spec() -> BoxedStrategy<NamelessMatchSpec> {
    match_spec().prop_map(Into::into).boxed()
}

macro_rules! impl_arbitrary {
    ($type:ty, $strategy:expr) => {
        impl Arbitrary for $type {
            type Parameters = ();
            type Strategy = BoxedStrategy<Self>;

            fn arbitrary_with((): Self::Parameters) -> Self::Strategy {
                $strategy
            }
        }
    };
}

impl_arbitrary!(PackageName, package_name());
impl_arbitrary!(PackageNameMatcher, package_name_matcher());
impl_arbitrary!(StringMatcher, string_matcher());
impl_arbitrary!(Channel, channel());
impl_arbitrary!(MatchSpecCondition, match_spec_condition(4));
impl_arbitrary!(MatchSpec, match_spec());
impl_arbitrary!(NamelessMatchSpec, nameless_match_spec());
