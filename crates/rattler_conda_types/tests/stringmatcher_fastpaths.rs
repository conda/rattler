use std::str::FromStr;

use rattler_conda_types::{
    CanonicalMatchSpecError, Flag, GenericVirtualPackage, MatchSpec, Matches, PackageName,
    PackageRecord, ParseMatchSpecOptions, RepodataRevision, StringMatcher, Version,
};

fn options() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict().with_repodata_revision(RepodataRevision::V3)
}

#[test]
fn parsed_matchspec_fastpaths_end_to_end() {
    for (pattern, matching, rejected) in [
        ("*", "", None),
        ("py*", "PY39_0", Some("xpy39_0")),
        ("*openblas", "x_OPENBLAS", Some("openblas_x")),
        ("é*", "éPY39", Some("ÉPY39")),
        ("*é", "PY39é", Some("PY39É")),
        ("py*blas", "PY39_OPENBLAS", Some("py39")),
    ] {
        let spec = MatchSpec::from_str(&format!("python[build='{pattern}']"), options()).unwrap();
        let record = PackageRecord::new(
            PackageName::new_unchecked("python"),
            Version::from_str("1.0").unwrap(),
            matching.into(),
        );
        assert!(spec.matches(&record), "{pattern:?} against {matching:?}");
        let nameless = spec.clone().into_nameless().1;
        assert!(nameless.matches(&record));
        let virtual_package = GenericVirtualPackage {
            name: record.name.clone(),
            version: record.version.clone().into(),
            build_string: record.build.clone(),
        };
        assert!(spec.matches(&virtual_package));
        if let Some(rejected) = rejected {
            let record = PackageRecord {
                build: rejected.into(),
                ..record
            };
            assert!(!spec.matches(&record));
            assert!(!nameless.matches(&record));
        }
        let canonical = spec.to_canonical_string().unwrap();
        assert_eq!(spec, MatchSpec::from_str(&canonical, options()).unwrap());
        assert_eq!(
            spec,
            serde_json::from_str::<MatchSpec>(&serde_json::to_string(&spec).unwrap()).unwrap()
        );
    }
}

#[test]
fn malformed_public_fastpath_variants_are_rejected() {
    for matcher in [
        StringMatcher::Prefix("".into()),
        StringMatcher::Suffix("".into()),
        StringMatcher::Prefix("a?".into()),
        StringMatcher::Suffix("[a]".into()),
        StringMatcher::Prefix("a/".into()),
        StringMatcher::Suffix("a\\".into()),
    ] {
        let spec = MatchSpec {
            build: Some(matcher),
            ..MatchSpec::from_str("python", options()).unwrap()
        };
        assert!(spec.to_canonical_string().is_err());
    }
}

#[test]
fn flags_fastpaths_end_to_end() {
    let spec = MatchSpec::from_str("python[flags=[cuda*, *blas]]", options()).unwrap();
    let mut record = PackageRecord::new(
        PackageName::new_unchecked("python"),
        Version::from_str("1.0").unwrap(),
        "".into(),
    );
    record.flags = vec![
        Flag::new_unchecked("CUDA12"),
        Flag::new_unchecked("openBLAS"),
    ];
    assert!(spec.matches(&record));
    record.flags.pop();
    assert!(!spec.matches(&record));
    let canonical = spec.to_canonical_string().unwrap();
    assert_eq!(spec, MatchSpec::from_str(&canonical, options()).unwrap());
}

#[test]
fn manually_constructed_simple_globs_are_rejected_as_noncanonical() {
    for pattern in ["*", "py*", "*blas", "é*", "*é"] {
        let matcher = StringMatcher::Glob(Box::new(glob::Pattern::new(pattern).unwrap()));
        let spec = MatchSpec {
            build: Some(matcher.clone()),
            ..MatchSpec::from_str("python", options()).unwrap()
        };
        assert!(matches!(
            spec.to_canonical_string(),
            Err(CanonicalMatchSpecError::UnrepresentableBuild(_))
        ));
        let spec = MatchSpec {
            flags: Some(vec![matcher]),
            ..MatchSpec::from_str("python", options()).unwrap()
        };
        assert!(matches!(
            spec.to_canonical_string(),
            Err(CanonicalMatchSpecError::UnrepresentableFlag(_))
        ));
    }
}
