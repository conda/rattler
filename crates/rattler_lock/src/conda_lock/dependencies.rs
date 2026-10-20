//! Dependency translation between solver specs and CEP-37 dependency mappings.
//!
//! CEP-37 stores dependencies as `name: constraint`, where the full spec is the
//! concatenation `name + " " + constraint`. Conda records keep a list of
//! `MatchSpec` strings and Python records keep PEP 508 requirements, so the
//! helpers here answer one question per dependency: does this spec survive that
//! representation unchanged? Anything that would change which packages a
//! dependency matches is an error, never a silent simplification.

use std::collections::BTreeMap;

use pep508_rs::{MarkerTree, Requirement, VersionOrUrl};
use rattler_conda_lock::NodePath;
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageNameMatcher, ParseStrictness, VersionSpec,
};

use super::error::{CondaLockError, CondaLockErrorKind};

/// One entry of a CEP-37 `dependencies` mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Dependency {
    /// The mapping key: the normalized dependency name.
    pub name: String,
    /// The mapping value: the constraint without the name, empty when the
    /// dependency is unconstrained.
    pub constraint: String,
}

/// The `dependencies` mapping of one package under construction.
///
/// Owns the rejection of repeated names: the source is a list, the destination
/// is a mapping, so inserting twice would drop a dependency.
#[derive(Debug, Default)]
pub(super) struct Dependencies {
    values: BTreeMap<String, String>,
    origins: BTreeMap<String, NodePath>,
}

impl Dependencies {
    /// Add one dependency, or fail if the package already has one by that name.
    pub(super) fn insert(
        &mut self,
        dependency: Dependency,
        path: NodePath,
    ) -> Result<(), CondaLockError> {
        let Dependency { name, constraint } = dependency;
        if let Some(origin) = self.origins.get(&name) {
            return Err(CondaLockError::new(
                path,
                CondaLockErrorKind::RepeatedDependencyName { name },
            )
            .with_related_path(origin.clone(), "first dependency with this name"));
        }
        self.origins.insert(name.clone(), path);
        self.values.insert(name, constraint);
        Ok(())
    }

    /// The finished mapping, in key order.
    pub(super) fn into_values(self) -> BTreeMap<String, String> {
        self.values
    }
}

/// Converts one conda `MatchSpec` string into a CEP-37 dependency entry.
///
/// Rejects specs whose meaning the CEP-37 spelling cannot carry: a name matcher
/// instead of an exact name, any field besides version and build (channel,
/// subdir, checksums, URL, license, ...), and specs that do not parse back to
/// themselves after being split into name and constraint.
pub(super) fn conda_spec(value: &str, path: &NodePath) -> Result<Dependency, CondaLockError> {
    let spec = MatchSpec::from_str(value, ParseStrictness::Lenient).map_err(|error| {
        CondaLockError::new(path.clone(), CondaLockErrorKind::InvalidMatchSpec(error))
    })?;
    let PackageNameMatcher::Exact(name) = &spec.name else {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::InexactDependencyName,
        ));
    };
    let supported = MatchSpec {
        name: spec.name.clone(),
        version: spec.version.clone(),
        build: spec.build.clone(),
        ..MatchSpec::default()
    };
    if spec != supported {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::UnsupportedSpecFields,
        ));
    }
    // CEP-37 concatenates name and constraint, so an unconstrained dependency
    // has an empty value rather than an explicit `*`.
    let unconstrained = spec.build.is_none()
        && spec
            .version
            .as_ref()
            .is_none_or(|version| *version == VersionSpec::Any);
    if unconstrained {
        return Ok(Dependency {
            name: name.as_normalized().to_owned(),
            constraint: String::new(),
        });
    }
    let constraint = NamelessMatchSpec::from(spec.clone()).to_string();
    let reconstructed = MatchSpec::from_str(
        &format!("{} {constraint}", name.as_source()),
        ParseStrictness::Lenient,
    )
    .map_err(|error| {
        CondaLockError::new(path.clone(), CondaLockErrorKind::InvalidMatchSpec(error))
    })?;
    if reconstructed != spec {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::LossyDependency,
        ));
    }
    Ok(Dependency {
        name: name.as_normalized().to_owned(),
        constraint,
    })
}

/// Converts one PEP 508 requirement into a CEP-37 dependency entry.
///
/// Rejects requirements whose installation semantics a bare constraint cannot
/// carry: extras, environment markers, and direct URLs.
pub(super) fn python_spec(
    requirement: &Requirement,
    path: &NodePath,
) -> Result<Dependency, CondaLockError> {
    if !requirement.extras.is_empty() || requirement.marker != MarkerTree::TRUE {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::PythonExtrasOrMarkers,
        ));
    }
    let constraint = match &requirement.version_or_url {
        None => String::new(),
        Some(VersionOrUrl::VersionSpecifier(spec)) => spec.to_string(),
        Some(VersionOrUrl::Url(_)) => {
            return Err(CondaLockError::new(
                path.clone(),
                CondaLockErrorKind::DirectPythonUrl,
            ));
        }
    };
    Ok(Dependency {
        name: requirement.name.to_string(),
        constraint,
    })
}
