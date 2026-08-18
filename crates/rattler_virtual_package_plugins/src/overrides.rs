//! Saying what a plugin would have reported, without running it.
//!
//! CEP 30 lets `CONDA_OVERRIDE_<NAME>` stand in for a virtual package the client
//! detects itself. A plugin's virtual packages want the same thing, for stronger
//! reasons: detecting one can mean solving an environment, installing it and
//! running a program that talks to hardware, so a developer reproducing a bug on
//! a machine without that hardware has no other way to get the name.
//!
//! One form, the one CEP 30 already defines: `CONDA_OVERRIDE_FOOBAR` stands in
//! for `__foobar`. There is nothing further to qualify. Only one plugin answers
//! for a name, so the name identifies the verdict on its own, and a variable
//! naming a channel as well would name a channel that cannot be in question.
//!
//! An override is read from a snapshot of the environment taken once per run, so
//! every plugin in one run agrees on what the overrides are, and so tests can
//! supply them without touching the process environment.

use std::collections::BTreeMap;

use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, ParseVersionError, SourcedVirtualPackage,
    Version, VirtualPackageSource,
};

/// The prefix CEP 30 gives these variables.
const PREFIX: &str = "CONDA_OVERRIDE_";

/// What an override says about one virtual package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overridden {
    /// The name is present, with this value.
    Present(Box<GenericVirtualPackage>),

    /// The variable was set to the empty string, which CEP 30 uses to mean the
    /// name is not there. A plugin claiming it is not run and reports nothing.
    Absent,
}

/// An override was set but could not be read.
#[derive(Debug, thiserror::Error)]
#[error("the environment variable '{variable}' does not describe a virtual package")]
pub struct OverrideError {
    /// The variable that was set.
    pub variable: String,

    /// Why its value could not be used.
    #[source]
    pub source: ParseVersionError,
}

/// The `CONDA_OVERRIDE_*` variables in effect for one run.
///
/// A snapshot rather than a live read of the environment: a run resolves several
/// plugins, possibly concurrently, and they should not be able to disagree about
/// what was set.
#[derive(Clone, Debug, Default)]
pub struct PluginOverrides {
    variables: BTreeMap<String, String>,
}

impl PluginOverrides {
    /// Takes the overrides from this process's environment.
    pub fn from_env() -> Self {
        Self::from_variables(std::env::vars())
    }

    /// Takes the overrides from `variables`, ignoring anything not named like an
    /// override. For tests, and for a caller that keeps its own environment.
    pub fn from_variables(variables: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            variables: variables
                .into_iter()
                .filter(|(name, _)| name.starts_with(PREFIX))
                .collect(),
        }
    }

    /// What the environment says about `name`, or `None` if it says nothing.
    pub fn get(&self, name: &PackageName) -> Option<Result<Overridden, OverrideError>> {
        let variable = variable_name(name);
        let value = self.variables.get(&variable)?;
        Some(parse(name, &variable, value))
    }

    /// The overrides that apply to `names`.
    ///
    /// A name missing from the result means the environment said nothing about
    /// it; a name mapped to [`Overridden::Absent`] means it said the name is not
    /// there. Those are different answers, which is why both are representable.
    ///
    /// When the result covers every name a plugin is on offer for, running that
    /// plugin cannot change the outcome and it is skipped.
    pub fn for_names<'a>(
        &self,
        names: impl IntoIterator<Item = &'a PackageName>,
    ) -> Result<BTreeMap<PackageName, Overridden>, OverrideError> {
        names
            .into_iter()
            .filter_map(|name| {
                let overridden = self.get(name)?;
                Some(overridden.map(|overridden| (name.clone(), overridden)))
            })
            .collect()
    }
}

/// Overrides that name a package, as virtual packages attributed to the plugin
/// they stand in for.
///
/// An [`Overridden::Absent`] contributes nothing, which is what it means. The
/// source is [`VirtualPackageSource::Overridden`] rather than
/// [`Plugin`](VirtualPackageSource::Plugin): the value is visible exactly where
/// the plugin's verdict would have been, but no environment was built and
/// nothing may claim otherwise.
pub fn sourced(
    overridden: BTreeMap<PackageName, Overridden>,
    channel: &ChannelUrl,
    plugin: &PackageName,
) -> Vec<SourcedVirtualPackage> {
    overridden
        .into_values()
        .filter_map(|overridden| match overridden {
            Overridden::Present(package) => Some(*package),
            Overridden::Absent => None,
        })
        .map(|package| SourcedVirtualPackage {
            source: VirtualPackageSource::Overridden {
                channel: channel.clone(),
                plugin: plugin.clone(),
            },
            package,
        })
        .collect()
}

/// `__foobar` -> `CONDA_OVERRIDE_FOOBAR`.
fn variable_name(name: &PackageName) -> String {
    format!(
        "{PREFIX}{}",
        shout(name.as_normalized().trim_start_matches('_'))
    )
}

/// Uppercased, with everything an environment variable cannot hold turned into
/// an underscore, so `__foobar-arch` reaches `FOOBAR_ARCH`.
fn shout(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `<version>`, or `<version>=<build string>` to set both.
fn parse(name: &PackageName, variable: &str, value: &str) -> Result<Overridden, OverrideError> {
    if value.is_empty() {
        return Ok(Overridden::Absent);
    }

    let (version, build_string) = value.split_once('=').unwrap_or((value, "0"));
    let version = version.parse::<Version>().map_err(|source| OverrideError {
        variable: variable.to_string(),
        source,
    })?;

    Ok(Overridden::Present(Box::new(GenericVirtualPackage {
        name: name.clone(),
        version,
        build_string: build_string.to_string(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(variables: &[(&str, &str)]) -> PluginOverrides {
        PluginOverrides::from_variables(
            variables
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        )
    }

    fn name(name: &str) -> PackageName {
        PackageName::new_unchecked(name)
    }

    fn present(overridden: Option<Result<Overridden, OverrideError>>) -> String {
        match overridden.expect("an override was set").expect("it parses") {
            Overridden::Present(package) => format!("{}={}", package.version, package.build_string),
            Overridden::Absent => "absent".to_string(),
        }
    }

    #[test]
    fn a_name_is_overridden_wherever_it_comes_from() {
        let overrides = overrides(&[("CONDA_OVERRIDE_FOOBAR", "1.2.3")]);
        assert_eq!(present(overrides.get(&name("__foobar"))), "1.2.3=0");
    }

    #[test]
    fn an_empty_value_means_the_name_is_absent() {
        let overrides = overrides(&[("CONDA_OVERRIDE_FOOBAR", "")]);
        assert_eq!(present(overrides.get(&name("__foobar"))), "absent");
    }

    #[test]
    fn a_build_string_can_be_set_too() {
        let overrides = overrides(&[("CONDA_OVERRIDE_FOOBAR_ARCH", "0=gen4")]);
        assert_eq!(present(overrides.get(&name("__foobar_arch"))), "0=gen4");
    }

    #[test]
    fn nothing_set_overrides_nothing() {
        let overrides = overrides(&[("PATH", "/usr/bin"), ("CONDA_OVERRIDE_OTHER", "1.0")]);
        assert!(overrides.get(&name("__foobar")).is_none());
    }

    #[test]
    fn a_value_that_is_not_a_version_is_an_error() {
        let overrides = overrides(&[("CONDA_OVERRIDE_FOOBAR", "not a version")]);
        let error = overrides
            .get(&name("__foobar"))
            .expect("an override was set")
            .expect_err("it does not parse");

        assert_eq!(error.variable, "CONDA_OVERRIDE_FOOBAR");
    }
}
