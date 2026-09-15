use std::collections::BTreeMap;

use pep508_rs::{MarkerTree, Requirement, VersionOrUrl};
use rattler_conda_lock::Error;
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageNameMatcher, ParseStrictness, VersionSpec,
};

pub(super) fn conda_spec(value: &str, path: &str) -> Result<(String, String), Error> {
    let spec = MatchSpec::from_str(value, ParseStrictness::Lenient)
        .map_err(|error| Error::new(path, error.to_string()))?;
    let PackageNameMatcher::Exact(name) = &spec.name else {
        return Err(Error::new(path, "dependency names must be exact"));
    };
    let supported = MatchSpec {
        name: spec.name.clone(),
        version: spec.version.clone(),
        build: spec.build.clone(),
        ..MatchSpec::default()
    };
    if spec != supported {
        return Err(Error::new(
            path,
            "CEP dependency values support only version and build constraints",
        ));
    }
    // CEP concatenates name and constraint, so an unconstrained dependency has an
    // empty value rather than an explicit `*`.
    let unconstrained = spec.build.is_none()
        && spec
            .version
            .as_ref()
            .is_none_or(|version| *version == VersionSpec::Any);
    if unconstrained {
        return Ok((name.as_normalized().to_owned(), String::new()));
    }
    let value = NamelessMatchSpec::from(spec.clone()).to_string();
    let reconstructed = MatchSpec::from_str(
        &format!("{} {value}", name.as_source()),
        ParseStrictness::Lenient,
    )
    .map_err(|error| Error::new(path, error.to_string()))?;
    if reconstructed != spec {
        return Err(Error::new(
            path,
            "dependency cannot be represented without changing its meaning",
        ));
    }
    Ok((name.as_normalized().to_owned(), value))
}

pub(super) fn python_spec(
    requirement: &Requirement,
    path: &str,
) -> Result<(String, String), Error> {
    if !requirement.extras.is_empty() || requirement.marker != MarkerTree::TRUE {
        return Err(Error::new(
            path,
            "CEP dependencies cannot preserve Python extras or environment markers",
        ));
    }
    let value = match &requirement.version_or_url {
        None => String::new(),
        Some(VersionOrUrl::VersionSpecifier(spec)) => spec.to_string(),
        Some(VersionOrUrl::Url(_)) => {
            return Err(Error::new(
                path,
                "CEP dependencies cannot preserve direct Python dependency URLs",
            ));
        }
    };
    Ok((requirement.name.to_string(), value))
}

pub(super) fn insert(
    result: &mut BTreeMap<String, String>,
    originals: &mut BTreeMap<String, String>,
    name: String,
    value: String,
    path: String,
) -> Result<(), Error> {
    if let Some(original) = originals.get(&name) {
        return Err(Error::new(
            &path,
            format!("repeated dependency name '{name}' cannot be represented in a CEP mapping"),
        )
        .with_related_path(original, "first dependency with this name"));
    }
    originals.insert(name.clone(), path);
    result.insert(name, value);
    Ok(())
}
