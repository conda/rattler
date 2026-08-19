use url::Url;

use rattler_conda_types::{
    CanonicalMatchSpecError, Channel, MatchSpec, MatchSpecCondition, ParseMatchSpecOptions,
    RepodataRevision, StringMatcher,
};

fn v3_options() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict().with_repodata_revision(RepodataRevision::V3)
}

fn assert_canonical_roundtrip(spec: &MatchSpec, options: ParseMatchSpecOptions) -> String {
    let canonical = spec.to_canonical_string().unwrap();
    let reparsed = MatchSpec::from_str(&canonical, options)
        .unwrap_or_else(|error| panic!("failed to parse canonical form {canonical}: {error}"));

    assert_eq!(reparsed, *spec, "canonical form: {canonical}");
    assert_eq!(reparsed.to_canonical_string().unwrap(), canonical);
    canonical
}

#[test]
fn canonical_fields_are_stably_ordered_and_roundtrip() {
    let options = v3_options();
    let spec = MatchSpec::from_str(
        r#"target[track_features="mkl debug",when="python >=3.11 and __linux",license_family="BSD",license="BSD-3-Clause",url="https://repo.example/pkg.conda#sha256:deadbeef",sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",md5="0123456789abcdef0123456789abcdef",namespace="python",subdir="linux-64",channel="https://repo.example/custom[linux-64,noarch]",flags=[cuda],extras=[docs],fn="pkg.conda",build_number=">=2",build="py*",version=">=1,<2"]"#,
        options,
    )
    .unwrap();

    let canonical = assert_canonical_roundtrip(&spec, options);
    assert!(canonical.starts_with("target["));
    assert!(!canonical.contains("::target"));

    let fields = [
        "version=",
        "build=",
        "build_number=",
        "fn=",
        "extras=",
        "flags=",
        "channel=",
        "subdir=",
        "namespace=",
        "md5=",
        "sha256=",
        "url=",
        "license=",
        "license_family=",
        "when=",
        "track_features=",
    ];
    let positions = fields.map(|field| canonical.find(field).unwrap());
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn canonical_conditions_use_only_required_parentheses() {
    let options = v3_options();
    for (condition, expected) in [
        ("a and b", "a and b"),
        ("a and b or c", "a and b or c"),
        ("a or b and c", "a or b and c"),
        ("(a or b) and c", "(a or b) and c"),
        ("a and (b and c)", "a and (b and c)"),
    ] {
        let source = format!(r#"target[when="{condition}"]"#);
        let spec = MatchSpec::from_str(&source, options).unwrap();
        let canonical = assert_canonical_roundtrip(&spec, options);
        assert!(
            canonical.contains(&format!(r#"when="{expected}""#)),
            "{canonical}"
        );
    }
}

#[test]
fn canonical_condition_leaves_support_v3_fields() {
    let options = v3_options();
    let spec = MatchSpec::from_str(
        r#"target[when="python[extras=[docs],flags=[cuda]] and __linux"]"#,
        options,
    )
    .unwrap();

    let canonical = assert_canonical_roundtrip(&spec, options);
    assert!(canonical.contains("python[extras=[docs],flags=[cuda]] and __linux"));
}

#[test]
fn canonical_condition_leaf_roundtrips_all_fields() {
    let options = v3_options();
    let leaf = MatchSpec::from_str(
        r#"python[version=">=3.11",build="py*",build_number=">=1",fn="python.conda",extras=[docs],flags=[cuda],channel="https://repo.example/custom[linux-64,noarch]",subdir="linux-64",namespace="python",md5="0123456789abcdef0123456789abcdef",sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",url="https://repo.example/python.conda#sha256:deadbeef",license="PSF-2.0",license_family="PSF",track_features="mkl"]"#,
        options,
    )
    .unwrap();
    let spec = MatchSpec {
        name: "target".parse().unwrap(),
        condition: Some(MatchSpecCondition::MatchSpec(Box::new(leaf))),
        ..MatchSpec::default()
    };

    assert_canonical_roundtrip(&spec, options);
}

#[test]
fn canonical_escaping_roundtrips() {
    let options = v3_options();
    for file_name in [
        r#"one \"double\" and 'single'"#,
        r#"one \'single\' and "double""#,
    ] {
        let leaf = MatchSpec {
            name: "python".parse().unwrap(),
            file_name: Some(file_name.to_string()),
            ..MatchSpec::default()
        };
        let spec = MatchSpec {
            name: "target".parse().unwrap(),
            condition: Some(MatchSpecCondition::MatchSpec(Box::new(leaf))),
            ..MatchSpec::default()
        };

        assert_canonical_roundtrip(&spec, options);
    }
}

#[test]
fn canonical_scalar_roundtrip_property() {
    let options = v3_options();
    let pieces = ["plain", "'", "\"", r#"\"#, r#"\'"#, r#"\""#];
    let mut successes = 0;

    for prefix in pieces {
        for suffix in pieces {
            let file_name = format!("{prefix}middle{suffix}.conda");
            let leaf = MatchSpec {
                name: "python".parse().unwrap(),
                file_name: Some(file_name),
                ..MatchSpec::default()
            };
            let spec = MatchSpec {
                name: "target".parse().unwrap(),
                condition: Some(MatchSpecCondition::MatchSpec(Box::new(leaf))),
                ..MatchSpec::default()
            };

            if spec.to_canonical_string().is_ok() {
                assert_canonical_roundtrip(&spec, options);
                successes += 1;
            }
        }
    }

    assert!(successes > 0);
}

#[test]
fn canonical_urls_do_not_serialize_credentials() {
    let secret_url = Url::parse(
        "https://user:password@prefix.dev/t/path-token/channel/pkg.conda?auth=session&keep=value#ticket=fragment-token",
    )
    .unwrap();
    let spec = MatchSpec {
        name: "target".parse().unwrap(),
        url: Some(secret_url.clone()),
        channel: Some(Channel::from_url(secret_url).into()),
        ..MatchSpec::default()
    };

    // Credentials are dropped rather than masked, leaving URLs that still
    // resolve: the same channel, reached without them.
    assert_eq!(
        spec.to_canonical_string().unwrap(),
        r#"target[channel="https://prefix.dev/channel/pkg.conda/",url="https://prefix.dev/channel/pkg.conda"]"#
    );
}

#[test]
fn canonical_url_fragments_roundtrip() {
    let options = v3_options();
    let spec = MatchSpec::from_str(
        r#"target[url="https://repo.example/pkg.conda#sha256:deadbeef"]"#,
        options,
    )
    .unwrap();
    assert_canonical_roundtrip(&spec, options);
}

#[test]
fn canonical_rejects_values_the_parser_cannot_preserve() {
    for value in [r#"both ' and " quote delimiters"#, r#"C:\cache\"#] {
        let spec = MatchSpec {
            name: "target".parse().unwrap(),
            file_name: Some(value.to_string()),
            ..MatchSpec::default()
        };
        assert_eq!(
            spec.to_canonical_string(),
            Err(CanonicalMatchSpecError::UnrepresentableScalar(
                value.to_string()
            ))
        );
    }

    for invalid in ["", "docs,tests"] {
        let spec = MatchSpec {
            name: "target".parse().unwrap(),
            extras: Some(vec![invalid.to_string()]),
            ..MatchSpec::default()
        };
        assert_eq!(
            spec.to_canonical_string(),
            Err(CanonicalMatchSpecError::UnrepresentableExtra(
                invalid.to_string()
            ))
        );
    }

    let spec = MatchSpec {
        name: "target".parse().unwrap(),
        flags: Some(vec![StringMatcher::Exact("cuda*".to_string())]),
        ..MatchSpec::default()
    };
    assert_eq!(
        spec.to_canonical_string(),
        Err(CanonicalMatchSpecError::UnrepresentableFlag(
            "cuda*".to_string()
        ))
    );
}

#[test]
fn canonical_rejects_nested_when() {
    let nested = MatchSpec {
        name: "python".parse().unwrap(),
        condition: Some(MatchSpecCondition::MatchSpec(Box::new(MatchSpec {
            name: "__linux".parse().unwrap(),
            ..MatchSpec::default()
        }))),
        ..MatchSpec::default()
    };
    let spec = MatchSpec {
        name: "target".parse().unwrap(),
        condition: Some(MatchSpecCondition::MatchSpec(Box::new(nested))),
        ..MatchSpec::default()
    };

    assert_eq!(
        spec.to_canonical_string(),
        Err(CanonicalMatchSpecError::NestedWhen)
    );
}

#[test]
fn canonical_rejects_ambiguous_condition_leaf() {
    let spec = MatchSpec {
        name: "target".parse().unwrap(),
        condition: Some(MatchSpecCondition::MatchSpec(Box::new(MatchSpec {
            name: "and".parse().unwrap(),
            ..MatchSpec::default()
        }))),
        ..MatchSpec::default()
    };

    assert_eq!(
        spec.to_canonical_string(),
        Err(CanonicalMatchSpecError::UnrepresentableConditionLeaf(
            "and".to_string()
        ))
    );
}

#[test]
fn display_remains_legacy() {
    let spec = MatchSpec::from_str(
        r#"target[version=">=1",build="py*",when="a and b"]"#,
        v3_options(),
    )
    .unwrap();

    assert_eq!(spec.to_string(), r#"target >=1 py*[when="a and b"]"#);
    assert_ne!(spec.to_string(), spec.to_canonical_string().unwrap());
}
