//! Working out which virtual packages a solve could actually ask for.
//!
//! Detecting a virtual package can mean installing an environment and running a
//! program that talks to hardware. Doing that for every plugin a channel happens
//! to register, when nothing in the solve mentions the names it speaks for, is
//! work with no possible effect on the answer.
//!
//! The channels' repodata is already in memory by the time a solve is being set
//! up, and every dependency a package could impose is written in it. Scanning it
//! for virtual package names is cheap, and it bounds what has to be detected:
//! a plugin whose names nothing mentions cannot change the outcome, so it does
//! not run.
//!
//! This is a *bound*, not an oracle. A name can be mentioned by a package the
//! solver would never have considered, and that plugin still runs. Narrowing it
//! further would mean asking the solver, which resolves candidates lazily behind
//! a runtime that cannot await a plugin.

use std::collections::BTreeSet;

use rattler_conda_types::PackageName;

/// Every virtual package name that could be asked for by any of `specs`.
///
/// `specs` is match spec text as it appears in repodata -- the `depends` and
/// `constrains` of the records a solve can see, plus whatever the user asked for
/// directly.
pub fn virtual_packages_mentioned<'a>(
    specs: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<PackageName> {
    specs
        .into_iter()
        .filter_map(virtual_package_name_of)
        .map(PackageName::new_unchecked)
        .collect()
}

/// The virtual package a single match spec constrains, if it constrains one.
///
/// Only the name is needed, so the spec is not parsed: a `MatchSpec` parse per
/// dependency string across a whole channel's repodata would cost more than the
/// detection this is meant to avoid. Everything that can end a name is treated
/// as ending it, so the worst a malformed spec can do is name a virtual package
/// that does not exist, which resolves to no factory and no work.
fn virtual_package_name_of(spec: &str) -> Option<&str> {
    // A channel-qualified spec puts the name after the channel and subdir.
    let spec = spec.trim();
    let spec = spec.rsplit("::").next()?.trim_start();

    if !spec.starts_with("__") {
        return None;
    }

    let end = spec
        .find(|c: char| c.is_whitespace() || "=<>!~,|([".contains(c))
        .unwrap_or(spec.len());
    (end > 2).then(|| &spec[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mentioned(specs: &[&str]) -> Vec<String> {
        virtual_packages_mentioned(specs.iter().copied())
            .into_iter()
            .map(|name| name.as_source().to_string())
            .collect()
    }

    #[test]
    fn finds_the_name_whatever_the_constraint_looks_like() {
        assert_eq!(
            mentioned(&[
                "__unix",
                "__glibc >=2.17",
                "__cuda>=12",
                "__rocm >=6.0,<7",
                "__osx >=11.0 *",
                "conda-forge::__cuda >=12",
                "__archspec 1 zen5",
                "__cuda_arch[version='>=8.0']",
                "__vendor_gpu=1.2.3=0",
            ]),
            [
                "__archspec",
                "__cuda",
                "__cuda_arch",
                "__glibc",
                "__osx",
                "__rocm",
                "__unix",
                "__vendor_gpu",
            ]
        );
    }

    #[test]
    fn ignores_everything_that_is_not_a_virtual_package() {
        assert!(
            mentioned(&[
                "python >=3.9",
                "numpy",
                "some_package",
                "_openmp_mutex",
                "conda-forge::python",
                "",
                "__",
            ])
            .is_empty()
        );
    }

    #[test]
    fn nothing_mentioned_is_an_empty_set() {
        assert!(mentioned(&[]).is_empty());
    }
}
